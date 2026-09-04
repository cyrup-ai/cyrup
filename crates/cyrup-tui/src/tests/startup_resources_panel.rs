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
    clippy::panic
)]

use std::path::PathBuf;

use crate::{
    App, DiagnosticSeverity, StartupDiagnostic, StartupReport, UiTheme, extension_diagnostics,
    resource_diagnostics,
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
