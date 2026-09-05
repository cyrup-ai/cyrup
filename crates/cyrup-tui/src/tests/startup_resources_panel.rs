//! TUI-006 — the startup loaded-resources / diagnostics panel.
//!
//! Pi's `showLoadedResources` (`interactive-mode.ts:1480-1690`) prints, at boot:
//!
//! * an inventory — `[Context]`, `[Skills]`, `[Prompts]`, `[Extensions]`, `[Themes]` — gated on
//!   `force || options.verbose || !getQuietStartup()` (`:1488`);
//! * four `warning`-styled diagnostic blocks — `[Skill conflicts]`, `[Prompt conflicts]`,
//!   `[Extension issues]`, `[Theme conflicts]` (`:1641-1690`) — which the boot call site asks for
//!   even under `quietStartup` (`{showDiagnosticsWhenQuiet: true}`, `:1769`).
//!
//! cyrup rendered NEITHER. The only startup row was `chrome::render_compact_hints`, so a skill
//! shadowed by a same-name one, a configured prompt path that does not exist, or an extension that
//! failed to instantiate produced no output at all — even though the session builder had the data
//! and was discarding it. `--verbose` meanwhile advertised "overrides quietStartup setting"
//! (`cli.rs:818`) while only raising the log level.
//!
//! These tests push the panel through the real `App::push_loaded_resources` seam and read the
//! committed scrollback — text AND the `warning`/`error` role colours — so they assert what the user
//! actually sees.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    // TUI-N02's arm-ordering guard slices the run-loop source, exactly as
    // `run_loop_swap_arm_reachable.rs` does.
    clippy::string_slice
)]

use std::path::PathBuf;

use crate::{
    App, DiagnosticSeverity, StartupDiagnostic, StartupReport, UiTheme, extension_diagnostics,
    resource_diagnostics, shortcut_diagnostics,
};
use cyrup_resources::{ResourceDiagnostic, ResourceKind};
use cyrup_session_svc::ExtensionLoadDiagnostic;
use ratatui::backend::TestBackend;
use ratatui::style::Style;

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap()
}

/// Push the panel through the real seam and return the committed scrollback.
fn commit(report: &StartupReport) -> (App<TestBackend>, String) {
    let mut app = new_app();
    app.push_loaded_resources(report);
    app.draw().unwrap();
    let out = app.scrollback_text();
    (app, out)
}

/// Whether some committed line containing `needle` is painted in `want`.
fn styled(app: &App<TestBackend>, needle: &str, want: Style) -> bool {
    app.scrollback_lines().iter().any(|line| {
        let joined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let effective = line
            .spans
            .iter()
            .find(|s| s.content.contains(needle))
            .map(|s| line.style.patch(s.style));
        joined.contains(needle)
            && effective.is_some_and(|st| st.fg == want.fg && st.add_modifier == want.add_modifier)
    })
}

fn loud_report() -> StartupReport {
    StartupReport {
        skills: vec!["review".into(), "deploy".into()],
        prompts: vec!["/plan".into()],
        extensions: vec!["cyrup-subagents".into()],
        themes: vec!["solarized".into()],
        context_files: vec!["~/proj/AGENTS.md".into()],
        ..Default::default()
    }
}

#[test]
fn the_loaded_inventory_reaches_the_scrollback() {
    let (_app, out) = commit(&loud_report());
    for needle in [
        "[Context]",
        "[Skills]",
        "[Prompts]",
        "[Extensions]",
        "[Themes]",
    ] {
        assert!(out.contains(needle), "`{needle}` section missing:\n{out}");
    }
    for needle in [
        "review",
        "deploy",
        "/plan",
        "cyrup-subagents",
        "solarized",
        "AGENTS.md",
    ] {
        assert!(out.contains(needle), "`{needle}` not listed:\n{out}");
    }
}

#[test]
fn quiet_startup_hides_the_inventory_but_still_shows_load_failures() {
    // Pi's boot call passes `showDiagnosticsWhenQuiet: true` (`:1769`), so `quietStartup` silences
    // the listing ONLY. This is the whole point of the item: a broken extension must not be silent.
    let report = StartupReport {
        quiet_startup: true,
        extension_diagnostics: vec![StartupDiagnostic::plain(
            DiagnosticSeverity::Error,
            Some("~/.cyrup/extensions/todo".into()),
            "world version mismatch",
        )],
        ..loud_report()
    };
    let (app, out) = commit(&report);
    assert!(
        !out.contains("[Skills]"),
        "quietStartup must hide the inventory:\n{out}"
    );
    assert!(
        out.contains("[Extension issues]"),
        "load failures survive quietStartup:\n{out}"
    );
    assert!(
        out.contains("~/.cyrup/extensions/todo"),
        "the failing path is named:\n{out}"
    );
    assert!(
        out.contains("world version mismatch"),
        "the reason is shown:\n{out}"
    );
    // Pi colours an `error`-type diagnostic with the `error` role (`:1470`).
    assert!(styled(
        &app,
        "world version mismatch",
        UiTheme::dark().error_style()
    ));
}

#[test]
fn verbose_overrides_quiet_startup_exactly_as_the_help_text_claims() {
    // `cli.rs:818` — "Force verbose startup (overrides quietStartup setting)".
    let report = StartupReport {
        quiet_startup: true,
        verbose: true,
        ..loud_report()
    };
    let (_app, out) = commit(&report);
    assert!(
        out.contains("[Skills]"),
        "--verbose must force the listing:\n{out}"
    );
}

#[test]
fn a_clean_quiet_startup_commits_nothing_at_all() {
    let report = StartupReport {
        quiet_startup: true,
        ..Default::default()
    };
    let (_app, out) = commit(&report);
    assert!(
        out.trim().is_empty(),
        "a silent boot must stay silent:\n{out:?}"
    );
}

#[test]
fn each_diagnostic_family_gets_pis_own_header() {
    let warn = |msg: &str| {
        StartupDiagnostic::plain(
            DiagnosticSeverity::Warning,
            Some("/p".into()),
            msg.to_string(),
        )
    };
    let report = StartupReport {
        quiet_startup: true,
        skill_diagnostics: vec![warn("bad skill")],
        prompt_diagnostics: vec![warn("bad prompt")],
        extension_diagnostics: vec![warn("bad extension")],
        theme_diagnostics: vec![warn("bad theme")],
        ..Default::default()
    };
    let (app, out) = commit(&report);
    // All four, including `[Theme conflicts]` (`:1684-1690`).
    for needle in [
        "[Skill conflicts]",
        "[Prompt conflicts]",
        "[Extension issues]",
        "[Theme conflicts]",
    ] {
        assert!(out.contains(needle), "`{needle}` block missing:\n{out}");
        assert!(
            styled(&app, needle, UiTheme::dark().warning_style()),
            "`{needle}` warning-styled"
        );
    }
}

#[test]
fn a_shadowed_skill_shows_the_winner_and_every_loser() {
    // A real `ResourceDiagnostic::collision` from the discovery pass, mapped and rendered.
    let diagnostics = vec![
        ResourceDiagnostic::collision(
            ResourceKind::Skill,
            "review",
            PathBuf::from("/home/u/proj/.cyrup/skills/review/SKILL.md"),
            PathBuf::from("/home/u/.cyrup/skills/review/SKILL.md"),
        ),
        ResourceDiagnostic::collision(
            ResourceKind::Skill,
            "review",
            PathBuf::from("/home/u/proj/.cyrup/skills/review/SKILL.md"),
            PathBuf::from("/home/u/pkg/skills/review/SKILL.md"),
        ),
        // A different family must NOT land in the skills block.
        ResourceDiagnostic::warning(ResourceKind::Prompt, "/gone", "prompt path does not exist"),
    ];
    let home = PathBuf::from("/home/u");
    let report = StartupReport {
        quiet_startup: true,
        skill_diagnostics: resource_diagnostics(&diagnostics, ResourceKind::Skill, Some(&home)),
        prompt_diagnostics: resource_diagnostics(&diagnostics, ResourceKind::Prompt, Some(&home)),
        ..Default::default()
    };
    assert_eq!(
        report.skill_diagnostics.len(),
        2,
        "diagnostics split by resource family"
    );
    assert_eq!(report.prompt_diagnostics.len(), 1);

    let (app, out) = commit(&report);
    assert!(
        out.contains("\"review\" collision:"),
        "grouped by name:\n{out}"
    );
    assert!(
        out.contains("✓ ~/proj/.cyrup/skills/review/SKILL.md"),
        "the winner is marked, home-shortened:\n{out}"
    );
    assert!(
        out.contains("✗ ~/.cyrup/skills/review/SKILL.md (skipped)"),
        "loser 1:\n{out}"
    );
    assert!(
        out.contains("✗ ~/pkg/skills/review/SKILL.md (skipped)"),
        "loser 2:\n{out}"
    );
    assert_eq!(
        out.matches("\"review\" collision:").count(),
        1,
        "one group, not two:\n{out}"
    );
    // The prompt warning lands under its OWN header, not the skills one.
    assert!(out.contains("[Prompt conflicts]"), "{out}");
    assert!(out.contains("prompt path does not exist"), "{out}");
    // Pi tints the collision winner's tick with `success` (`:1451`).
    assert!(styled(&app, "✓", UiTheme::dark().success_style()));
}

#[test]
fn extension_load_errors_map_from_the_session_services_shape() {
    // Exactly what `AgentSessionServices::startup_diagnostics.extensions` now carries.
    let errors = vec![ExtensionLoadDiagnostic {
        path: PathBuf::from("/home/u/.cyrup/extensions/broken"),
        error: "untrusted project-local extension".to_string(),
        // The trust skip is reported in the panel but is NOT the fatal load-failure class.
        fatal: false,
    }];
    let home = PathBuf::from("/home/u");
    let report = StartupReport {
        quiet_startup: true,
        extension_diagnostics: extension_diagnostics(&errors, Some(&home)),
        ..Default::default()
    };
    let (_app, out) = commit(&report);
    assert!(out.contains("[Extension issues]"), "{out}");
    assert!(out.contains("~/.cyrup/extensions/broken"), "{out}");
    assert!(out.contains("untrusted project-local extension"), "{out}");
}

/// EXT-S01: a NATIVE built-in whose `init()` failed is now contained by the session builder and
/// recorded on the same channel — but keyed by its extension ID, not an on-disk path (a native has
/// none). The panel must still name it and its error. Without this the containment fix would be a
/// silent swallow, which is strictly worse than the abort it replaced.
#[test]
fn a_contained_native_init_failure_renders_under_extension_issues() {
    let errors = vec![ExtensionLoadDiagnostic {
        path: PathBuf::from("permission-system"),
        error: "extension panicked: policy file unreadable".to_string(),
        fatal: true,
    }];
    let report = StartupReport {
        // Under `quietStartup` the LISTING is suppressed but the diagnostics are not
        // (`showDiagnosticsWhenQuiet: true`, interactive-mode.ts:1769) — the case that matters,
        // since a user who never sees the inventory must still see the failure.
        quiet_startup: true,
        extension_diagnostics: extension_diagnostics(&errors, Some(&PathBuf::from("/home/u"))),
        ..Default::default()
    };
    let (_app, out) = commit(&report);
    assert!(out.contains("[Extension issues]"), "{out}");
    assert!(
        out.contains("permission-system"),
        "the failing extension must be named:\n{out}"
    );
    assert!(out.contains("policy file unreadable"), "{out}");
}

/// **The L2 defect, surviving in `startup_lines`.**
///
/// Every child upstream adds to `loadedResourcesContainer` is a `new Text(…, 0, 0)`
/// (`interactive-mode.ts:1766`, `:1775`, `:1798`, `:1807`) or an `ExpandableText(…, 0, 0)`
/// (`:1626-1632`, which `extends Text`), and `Text.render` WRAPS at
/// `contentWidth = Math.max(1, width - paddingX * 2)` (`text.ts:64`) BEFORE prefixing `leftMargin`
/// to each produced row (`:70-76`).
///
/// `startup_lines` took no `width` at all: it inserted the margin into the LOGICAL row and handed
/// the result to the outer `Paragraph::wrap`, which reflowed it at the FULL frame width. A single
/// extension diagnostic carrying an absolute path drew rows of 77 and 73 columns inside a
/// 40-column frame, with every continuation row flush at column 0 — the precise shape this batch
/// existed to eliminate, still standing in the one block nobody had converted.
#[test]
fn startup_panel_rows_wrap_inside_the_frame() {
    const W: u16 = 40;
    let long_path = "/home/somebody/workspace/project/.cyrup/agent/extensions/very-long-name.json";
    let long_msg = "failed to instantiate: the module exported no `activate` entry point and the \
                    manifest declared an unknown capability";
    let report = StartupReport {
        quiet_startup: true,
        extension_diagnostics: vec![StartupDiagnostic::plain(
            DiagnosticSeverity::Error,
            Some(long_path.into()),
            long_msg,
        )],
        ..Default::default()
    };

    let mut app = App::new(TestBackend::new(W, 24), UiTheme::dark()).unwrap();
    app.push_loaded_resources(&report);
    app.draw().unwrap();
    let lines = app.scrollback_lines();
    assert!(!lines.is_empty(), "panel committed nothing");

    for line in lines {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            line.width() <= usize::from(W),
            "row {:?} is {} cells in a {W}-column frame",
            text,
            line.width()
        );
    }
    // The wrap actually happened: the long path and the long message each occupy several rows.
    assert!(
        lines.len() > 4,
        "nothing wrapped: {:?}",
        app.scrollback_text()
    );
    assert!(
        app.scrollback_text().contains("very-long-name.json"),
        "path lost in the wrap"
    );
    assert!(
        app.scrollback_text().contains("capability"),
        "message tail lost in the wrap"
    );

    // MIRROR — a short panel at a wide frame is untouched, margin and all: `[Skills]` still opens
    // at column `outputPad` and the indented list rows keep their own two-space inset.
    let mut wide = App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap();
    wide.push_loaded_resources(&loud_report());
    wide.draw().unwrap();
    let text = wide.scrollback_text();
    assert!(
        text.lines().any(|l| l.trim_start().starts_with("[Skills]")),
        "{text}"
    );
    assert!(
        text.lines().any(|l| l == "   deploy"),
        "list row lost its own inset:\n{text}"
    );
    assert!(
        text.lines().any(|l| l == "   review"),
        "list row lost its own inset:\n{text}"
    );
    for line in wide.scrollback_lines() {
        assert!(
            line.width() <= 120,
            "wide frame overflow: {:?}",
            line.width()
        );
    }
}

/// EXT-039 — the extension-shortcut conflict warnings reach the SAME `[Extension issues]` block the
/// load failures do, appended after them.
///
/// Upstream builds one `extensionDiagnostics` vector: load errors first, then the command
/// diagnostics, then `extensionRunner.getShortcutDiagnostics()`, and renders the lot under one
/// header (`modes/interactive/interactive-mode.ts:1872-1892` @v0.84.4). A shortcut warning is
/// `{type: "warning", …}` (`extensions/runner.ts:549-553`), so it must paint as a WARNING beside an
/// error-painted load failure.
///
/// RED before this pass: `shortcut_diagnostics` had no projector and no caller — the registry
/// recorded the warnings and nothing ever read them, which is the "emit no conflict diagnostics"
/// half of the item.
#[test]
fn shortcut_conflict_warnings_join_the_extension_issues_block() {
    let load_failure = extension_diagnostics(
        &[ExtensionLoadDiagnostic {
            path: PathBuf::from("/x/broken.wasm"),
            error: "instantiate failed".into(),
            fatal: true,
        }],
        None,
    );
    let mut extension = load_failure;
    extension.extend(shortcut_diagnostics(&[cyrup_ext::ExtensionConflict {
        path: "ext-a".into(),
        message: "Extension shortcut 'ctrl+c' from ext-a conflicts with built-in shortcut. \
                  Skipping."
            .into(),
    }]));

    let report = StartupReport {
        quiet_startup: true,
        extension_diagnostics: extension,
        ..Default::default()
    };
    let (app, out) = commit(&report);

    assert_eq!(
        out.matches("[Extension issues]").count(),
        1,
        "one block, not two:\n{out}"
    );
    assert!(out.contains("instantiate failed"), "{out}");
    assert!(
        out.contains("conflicts with built-in shortcut. Skipping."),
        "the refusal warning never reached the panel:\n{out}"
    );
    assert!(
        out.find("instantiate failed") < out.find("conflicts with built-in"),
        "pi appends the shortcut diagnostics after the load errors:\n{out}"
    );
    assert!(
        styled(
            &app,
            "conflicts with built-in",
            UiTheme::dark().warning_style()
        ),
        "a shortcut conflict is a WARNING, not an error:\n{out}"
    );
}

// ================================================================================ TUI-N02 =======
//
// The panel had exactly ONE production call site — the boot path in `crates/cyrup/src/interactive.rs`
// — and its builder was a private function in that binary. pi emits it from TWO places, both with
// the identical `{force: false, showDiagnosticsWhenQuiet: true}` options object:
//
// ```ts
// // pi v0.84.4 coding-agent/src/modes/interactive/interactive-mode.ts:1981-1982
// const extensionRunner = this.session.extensionRunner;
// this.setupExtensionShortcuts(extensionRunner);
// this.showLoadedResources({ force: false, showDiagnosticsWhenQuiet: true });
//
// // …:5990-5994, inside handleReloadCommand
// const runner = this.session.extensionRunner;
// this.setupExtensionShortcuts(runner);
// this.showLoadedResources({
//     force: false,
//     showDiagnosticsWhenQuiet: true,
// });
// ```
//
// The first sits in `bindCurrentSessionExtensions`, which `rebindCurrentSession` calls on boot AND
// on every session replacement (the runtime's `setRebindSession` hook, `:576-578`). So a `/reload`
// that broke an extension, shadowed a skill or introduced a prompt conflict re-collected all of
// that server-side and cyrup discarded it, printing only `Reloaded keybindings, extensions, …`.

/// A native built-in whose `init()` always fails, so `SessionBuilder::build` records a contained
/// extension load failure in `startup_diagnostics.extensions` — the channel
/// `StartupReport::from_session` reads for `[Extension issues]` (the same trigger
/// `cyrup-session-svc`'s `build_containment_and_flag_diagnostics.rs` uses).
struct FailingExt;

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for FailingExt {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("broken-ext")
    }
    async fn init(&self, _api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        Err(cyrup_ext::ExtError::Panicked(
            "boom: the reload broke this extension".to_string(),
        ))
    }
    async fn on_event(
        &self,
        _ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        cyrup_ext::HookOutcome::Noop
    }
}

/// A session whose effective settings carry `quietStartup: <quiet>` and whose build recorded one
/// contained extension load failure.
async fn broken_extension_session(
    dir: &std::path::Path,
    quiet: bool,
) -> std::sync::Arc<cyrup_session_svc::AgentSession> {
    let cwd = dir.join("project");
    let agent_dir = dir.join("agent");
    let home = dir.join("home");
    for d in [&cwd, &agent_dir, &home] {
        std::fs::create_dir_all(d).unwrap();
    }
    // One `[Context]` entry, so the listing half of the panel has something to print.
    std::fs::write(cwd.join("AGENTS.md"), "# house rules\n").unwrap();
    let faux: std::sync::Arc<dyn cyrup_provider::Provider> =
        std::sync::Arc::new(cyrup_provider::faux::FauxProvider::new());
    let mut cfg = cyrup_session_svc::SessionConfig::new(cwd, agent_dir);
    cfg.home = home;
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    std::sync::Arc::new(
        cyrup_session_svc::SessionBuilder::new(faux, cfg)
            // pi's `settingsManager.applyOverrides` tier (CFG-059) — the merged view the panel
            // reads, without a settings file for the fixture to keep in sync.
            .cli_settings(
                cyrup_config::settings::Settings::parse(&format!("{{\"quietStartup\": {quiet}}}"))
                    .unwrap(),
            )
            .with_native_extension(
                std::sync::Arc::new(FailingExt) as std::sync::Arc<dyn cyrup_ext::NativeExtension>
            )
            .build()
            .await
            .unwrap(),
    )
}

/// The item's own scenario: the panel is derivable from a bare session, so the run loop can re-emit
/// it, and a `quietStartup` user still gets the diagnostics — pi's `showDiagnosticsWhenQuiet: true`
/// (`:1986`, `:5993`) against `showListing = force || verbose || !quietStartup` (`:1702`).
///
/// RED before this pass: there was no session-derived seam at all. `build_startup_report` was
/// private to `crates/cyrup/src/interactive.rs`, so nothing in this crate — where the run loop
/// lives — could build the report, which is exactly why `/reload` had nothing to push.
#[tokio::test]
async fn the_panel_is_derivable_from_a_session_and_survives_quiet_startup() {
    let dir = tempfile::tempdir().unwrap();
    let session = broken_extension_session(dir.path(), true).await;
    assert_eq!(
        session.services().startup_diagnostics.extensions.len(),
        1,
        "fixture must actually have recorded a load failure"
    );

    let mut app = new_app();
    app.push_session_loaded_resources(&session);
    app.draw().unwrap();
    let out = app.scrollback_text();

    assert!(
        out.contains("[Extension issues]"),
        "the diagnostics block must survive quietStartup:\n{out}"
    );
    assert!(
        out.contains("boom: the reload broke this extension"),
        "the recorded failure never reached the panel:\n{out}"
    );
    // `showListing` is false, so the inventory is suppressed — the half `quietStartup` DOES hide.
    assert!(
        !out.contains("[Skills]") && !out.contains("[Context]"),
        "quietStartup must still hide the inventory:\n{out}"
    );
}

/// `--verbose` is pi's `options.verbose`, read inside `showListing` (`:1702`) — so it has to be
/// held on the App, not passed at the single boot call site, or the re-emit could never honour it.
#[tokio::test]
async fn set_verbose_startup_overrides_quiet_startup_at_the_session_seam() {
    let dir = tempfile::tempdir().unwrap();
    let session = broken_extension_session(dir.path(), true).await;

    let mut app = new_app();
    app.set_verbose_startup(true);
    app.push_session_loaded_resources(&session);
    app.draw().unwrap();
    let out = app.scrollback_text();

    assert!(
        out.contains("[Context]"),
        "--verbose must force the listing through the session seam too:\n{out}"
    );
}

/// The other half of the item: the panel must actually be pushed from the run loop's session-swap
/// arm — the arm every `/reload`, `/new`, `/resume`, `/fork` and `/import` funnels through — and it
/// must be pushed in pi's ORDER.
///
/// Read from the source for the same reason `run_loop_swap_arm_reachable.rs` does: nothing in this
/// crate constructs a `RunCtx` (it owns a runtime, an event stream and nine channels), so the arm's
/// internal ordering has no behavioural coverage. Two orderings are load-bearing and neither is
/// observable from the seam test above:
///
/// * `install_extension_shortcuts` BEFORE the push — `resolve_shortcut_specs` is what records the
///   EXT-039 reserved-key refusals that `[Extension issues]` renders, and upstream orders the pair
///   the same way at both call sites (`:1981-1982`, `:5990-5991`).
/// * the push BEFORE the replay — pi's `loadedResourcesContainer` is pinned above `chatContainer`
///   (`:594-596`), and cyrup's committed scrollback is linear.
#[test]
fn the_session_swap_arm_pushes_the_panel_after_the_shortcuts_and_before_the_replay() {
    const ARMS_SRC: &str = include_str!("../app/run_arms.rs");
    let offset = ARMS_SRC
        .find("pub(crate) async fn on_session_swapped")
        .expect("run_arms.rs must still define `on_session_swapped`");
    let body = &ARMS_SRC[offset..];
    let end = body
        .find("pub(crate) fn drain_over_budget_arm")
        .unwrap_or(body.len());
    let arm = &body[..end];

    let push = arm
        .find("self.push_session_loaded_resources(")
        .unwrap_or_else(|| {
            panic!(
                "the `session_swapped` arm never pushes the loaded-resources panel — `/reload` \
             swallows the diagnostics it just re-collected (pi `showLoadedResources` at \
             interactive-mode.ts:1982 and :5991 @v0.84.4)"
            )
        });
    let shortcuts = arm
        .find("self.install_extension_shortcuts(")
        .unwrap_or_else(|| panic!("the `session_swapped` arm must re-source extension shortcuts"));
    let replay = arm
        .find(".replay_items()")
        .unwrap_or_else(|| panic!("the `session_swapped` arm must still replay the conversation"));

    assert!(
        shortcuts < push,
        "`install_extension_shortcuts` must precede the panel push: it is what RECORDS the \
         EXT-039 reserved-key refusals the `[Extension issues]` block renders \
         (interactive-mode.ts:1981-1982 @v0.84.4)"
    );
    assert!(
        push < replay,
        "the panel must be pushed BEFORE the replay: pi's `loadedResourcesContainer` is pinned \
         above `chatContainer` (interactive-mode.ts:594-596) and cyrup's committed scrollback is \
         linear, so a push after the replay would land under the conversation"
    );
}
