//! TUI-149 — the surfaces that name a theme learn the `system` theme: the extension theme API, the
//! HTML export, and the terminal's own appearance notifications (mode `2031`).
//!
//! pi v1.0.0 sources: `ExtensionUIContext.getAllThemes` / `getTheme` / `setTheme`
//! (`interactive-mode.ts:2615-2631`, `theme.ts:467-498`, `:642-648`), `getResolvedThemeColors` and
//! `getThemeExportColors` (`theme.ts:903-950`), and the controller's `applyTerminalColorScheme
//! Change` (`theme-controller.ts:240-248`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use std::sync::Arc;

use cyrup_ext::host::HostServices;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_resources::color::Rgb;
use cyrup_resources::{Appearance, ResourceRegistry, ResourceSet, builtin_themes};
use cyrup_session_svc::{ExportState, LiveHostServices, session_jsonl_to_html_with_theme};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::theme_access::ThemeSwitch;
use crate::{App, ColorMode, TerminalColors, TerminalTheme, ThemeController, UiTheme};

const JSONL: &str = "{\"type\":\"session\",\"id\":\"s\"}\n{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n";

/// A Catppuccin-Mocha-like terminal: every token the system theme generates is a concrete colour.
pub(super) fn mocha() -> TerminalColors {
    let palette: [Rgb; 16] = [
        Rgb::new(0x45, 0x47, 0x5a),
        Rgb::new(0xf3, 0x8b, 0xa8),
        Rgb::new(0xa6, 0xe3, 0xa1),
        Rgb::new(0xf9, 0xe2, 0xaf),
        Rgb::new(0x89, 0xb4, 0xfa),
        Rgb::new(0xf5, 0xc2, 0xe7),
        Rgb::new(0x94, 0xe2, 0xd5),
        Rgb::new(0xba, 0xc2, 0xde),
        Rgb::new(0x58, 0x5b, 0x70),
        Rgb::new(0xf3, 0x8b, 0xa8),
        Rgb::new(0xa6, 0xe3, 0xa1),
        Rgb::new(0xf9, 0xe2, 0xaf),
        Rgb::new(0x89, 0xb4, 0xfa),
        Rgb::new(0xf5, 0xc2, 0xe7),
        Rgb::new(0x94, 0xe2, 0xd5),
        Rgb::new(0xa6, 0xad, 0xc8),
    ];
    TerminalColors {
        foreground: Some(Rgb::new(0xcd, 0xd6, 0xf4)),
        background: Some(Rgb::new(0x1e, 0x1e, 0x2e)),
        palette: Some(palette),
    }
}

/// What a terminal answers to the colour batch: OSC 10, OSC 11, sixteen OSC 4 and the DA1.
pub(super) fn terminal_reply(colors: &TerminalColors) -> Vec<u8> {
    let osc = |rgb: Rgb| {
        format!(
            "rgb:{:02x}{:02x}/{:02x}{:02x}/{:02x}{:02x}",
            rgb.r, rgb.r, rgb.g, rgb.g, rgb.b, rgb.b
        )
    };
    let mut out = String::new();
    if let Some(fg) = colors.foreground {
        out.push_str(&format!("\x1b]10;{}\x07", osc(fg)));
    }
    if let Some(bg) = colors.background {
        out.push_str(&format!("\x1b]11;{}\x07", osc(bg)));
    }
    if let Some(palette) = colors.palette {
        for (index, rgb) in palette.iter().enumerate() {
            out.push_str(&format!("\x1b]4;{index};{}\x07", osc(*rgb)));
        }
    }
    out.push_str("\x1b[?62;22c");
    out.into_bytes()
}

fn registry() -> Arc<ResourceRegistry> {
    Arc::new(ResourceRegistry {
        themes: ResourceSet::build(builtin_themes()),
        ..ResourceRegistry::default()
    })
}

fn services() -> Arc<LiveHostServices> {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    Arc::new(LiveHostServices::new(
        provider,
        cyrup_tools::Backend::default().proc,
        std::env::temp_dir(),
    ))
}

/// An app booted on the system theme, with the seams an extension reads installed as `App::run`
/// installs them.
fn wired() -> (
    App<TestBackend>,
    Arc<LiveHostServices>,
    tokio::sync::mpsc::UnboundedReceiver<ThemeSwitch>,
) {
    let controller = ThemeController::boot(None, ColorMode::TrueColor, TerminalTheme::Dark);
    let mut app = App::new(TestBackend::new(80, 24), controller.theme()).unwrap();
    app.set_theme_controller(controller);
    let svc = services();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    app.install_extension_readbacks(&svc, registry(), tx);
    (app, svc, rx)
}

fn accent_of(doc: &serde_json::Value) -> String {
    doc["colors"]["accent"].as_str().unwrap().to_string()
}

// ---------------------------------------------------------------------------------------------
// Item 1 — the extension theme API
// ---------------------------------------------------------------------------------------------

/// `getAllThemes()` lists `system` first with no path (`theme.ts:479-480`, `:494-497`); a custom
/// theme of the same name is not listed twice (`seen`, `:471-477`).
///
/// FAILS without the change: `list` returned only `dark` and `light`.
#[test]
fn system_is_listed_first_with_no_path_and_never_twice() {
    let (_app, svc, _rx) = wired();
    let listed = HostServices::theme_list(svc.as_ref());
    let rows = listed.as_array().unwrap();
    assert_eq!(rows[0]["name"], "system");
    assert!(rows[0]["path"].is_null());
    let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(names, vec!["system", "dark", "light"]);
}

/// `getTheme("system")` loads the generated theme without switching to it (`theme.ts:642-648`,
/// `loadTheme` `:633`): the document carries the tokens generated from the colours the terminal
/// reported, not a name lookup that misses.
///
/// FAILS without the change: `by_name("system")` was `None`, so `theme-get-json` on the system
/// theme — the default one — answered `None` for every guest.
#[test]
fn a_guest_reads_the_generated_system_theme_by_name_and_as_the_active_theme() {
    let (mut app, svc, _rx) = wired();
    let resources = registry();
    app.apply_terminal_colors(mocha(), &resources);
    app.draw().unwrap();

    let doc = HostServices::theme_by_name(svc.as_ref(), "system").expect("`system` loads");
    assert_eq!(doc["name"], "system");
    let accent = accent_of(&doc);
    assert!(
        accent.starts_with('#') && accent.len() == 7,
        "a terminal that reported its palette generates concrete colours: {accent}"
    );
    // The same colour the interface paints.
    let Some(Color::Rgb(r, g, b)) = app.state().theme.accent else {
        panic!("the painted system theme has a concrete accent");
    };
    assert_eq!(accent, format!("#{r:02x}{g:02x}{b:02x}"));

    // `live.rs::theme_get_json`'s composition: the ACTIVE theme's colours.
    let composed = HostServices::theme(svc.as_ref())
        .and_then(|name| HostServices::theme_by_name(svc.as_ref(), &name))
        .expect("the active theme is readable");
    assert_eq!(composed, doc);
    assert_eq!(
        HostServices::theme(svc.as_ref()),
        Some("system".to_string())
    );
}

/// A terminal that reported nothing gets pi's tier-3 theme: ANSI indices and `""`.
#[test]
fn the_system_document_before_any_report_is_ansi_indices_and_defaults() {
    let (mut app, svc, _rx) = wired();
    // The query timed out: the grayscale of the first frame ends, nothing was reported.
    app.apply_terminal_colors(TerminalColors::default(), &registry());
    app.draw().unwrap();
    let doc = HostServices::theme_by_name(svc.as_ref(), "system").unwrap();
    assert_eq!(doc["colors"]["accent"], 5, "violet is palette slot 5");
    assert_eq!(
        doc["colors"]["userMessageBg"], "",
        "a panel has no background"
    );
}

/// The document follows the terminal when its colours change (the generation moved).
#[test]
fn the_system_document_is_regenerated_when_the_terminal_colours_change() {
    let (mut app, svc, _rx) = wired();
    let resources = registry();
    app.draw().unwrap();
    let before = HostServices::theme_by_name(svc.as_ref(), "system").unwrap();
    app.apply_terminal_colors(mocha(), &resources);
    app.draw().unwrap();
    let after = HostServices::theme_by_name(svc.as_ref(), "system").unwrap();
    assert_ne!(accent_of(&before), accent_of(&after));
}

/// `setTheme("system")` succeeds and asks the run loop for the generated theme; an unknown name is
/// still pi's `Theme not found` (`theme.ts:622`).
///
/// FAILS without the change: `set("system")` was `Err("Theme not found: system")`.
#[test]
fn a_guest_can_switch_to_the_system_theme() {
    let (mut app, svc, mut rx) = wired();
    app.draw().unwrap();
    assert_eq!(HostServices::set_theme(svc.as_ref(), "system"), Ok(()));
    let request = rx.try_recv().expect("the switch reaches the run loop");
    assert!(matches!(request, ThemeSwitch::System), "{request:?}");

    assert_eq!(
        HostServices::set_theme(svc.as_ref(), "no-such-theme"),
        Err("Theme not found: no-such-theme".to_string())
    );
    assert!(rx.try_recv().is_err());
}

/// The run loop paints the system theme from the controller's colours, seats the name, and the
/// colour-scheme notifications follow (`setAutoSync(themeName === SYSTEM_THEME_NAME)`).
#[test]
fn applying_a_system_switch_paints_the_generated_theme() {
    let mut controller =
        ThemeController::boot(Some("dark"), ColorMode::TrueColor, TerminalTheme::Dark);
    controller.apply_terminal_colors(mocha());
    let mut app = App::new(TestBackend::new(80, 24), controller.theme()).unwrap();
    app.set_theme_controller(controller);
    assert_eq!(app.state().theme.name, "dark");

    let name = app.apply_theme_switch(ThemeSwitch::System);
    assert_eq!(name, "system");
    assert_eq!(app.state().theme.name, "system");
    assert!(
        matches!(app.state().theme.accent, Some(Color::Rgb(..))),
        "generated from the reported palette: {:?}",
        app.state().theme.accent
    );
    assert_eq!(app.theme_controller().unwrap().active_name(), "system");
}

// ---------------------------------------------------------------------------------------------
// Item 3 — HTML export
// ---------------------------------------------------------------------------------------------

/// pi's `getResolvedThemeColors()` reads the CURRENT theme's colours, so an export of the system
/// theme carries the generated palette (`theme.ts:903-906`), and its backdrops are derived from
/// `userMessageBg` because the generated theme has no `export` block (`:924-925`).
///
/// FAILS without the change: the name `system` matched no document and the export fell back to the
/// compiled-in `dark` palette.
#[test]
fn an_export_of_the_system_theme_carries_the_generated_palette() {
    let (mut app, svc, _rx) = wired();
    app.apply_terminal_colors(mocha(), &registry());
    app.draw().unwrap();

    let export = svc.export_theme().expect("the TUI answers the export");
    let html = session_jsonl_to_html_with_theme(JSONL, &export, &ExportState::from_file());

    let Some(Color::Rgb(r, g, b)) = app.state().theme.accent else {
        panic!("generated accent");
    };
    assert!(
        html.contains(&format!("--accent: #{r:02x}{g:02x}{b:02x};")),
        "the generated accent reaches the stylesheet"
    );
    // The dark built-in's accent, which the old fallback exported, is not what is there now.
    let dark = cyrup_session_svc::ExportTheme::default();
    let dark_html = session_jsonl_to_html_with_theme(JSONL, &dark, &ExportState::from_file());
    assert_ne!(html, dark_html);

    // Backdrops derive from the generated `userMessageBg` (no `export` block).
    let user_bg = export.role("userMessageBg").unwrap();
    let derived = cyrup_session_svc::derive_export_colors(Some(user_bg));
    assert_eq!(export.backdrops(), derived);
}

/// Tokens a generated theme leaves to the terminal (`""`) export as the terminal's reported
/// default colours, a dim token as the mix toward the background (`Theme.colors`,
/// `theme.ts:321-338`) — here the tier where the terminal reported colours but no palette.
#[test]
fn the_exported_defaults_are_the_terminals_reported_colours() {
    let (mut app, svc, _rx) = wired();
    // No palette, no background: pi's tier 3 — neutral tokens are `""`, and dim.
    app.apply_terminal_colors(
        TerminalColors {
            foreground: Some(Rgb::new(0xcd, 0xd6, 0xf4)),
            ..Default::default()
        },
        &registry(),
    );
    app.draw().unwrap();
    let export = svc.export_theme().unwrap();
    let text = export.role("text").unwrap();
    assert_eq!(
        (text.r(), text.g(), text.b()),
        (0xcd, 0xd6, 0xf4),
        "`text` is the terminal default foreground, which the terminal reported"
    );
    // `muted` is a faint neutral: 40% of the way from the foreground to the (guessed) background.
    let muted = export.role("muted").unwrap();
    assert!(
        muted.r() < text.r() && muted.g() < text.g() && muted.b() < text.b(),
        "a dim token is mixed toward the background: {muted} vs {text}"
    );
}

/// The same getter fills `""` in a THEME DOCUMENT: a custom theme's background token exports as the
/// terminal's background, not its foreground (v0.84 exported the default text colour for both).
#[test]
fn a_document_theme_exports_its_empty_tokens_from_the_terminals_defaults() {
    let doc: cyrup_resources::theme::ThemeData = serde_json::from_str(
        r##"{"name":"quiet","colors":{"text":"","userMessageBg":"","accent":"#ff0000"}}"##,
    )
    .unwrap();
    let theme = cyrup_resources::Theme {
        key: cyrup_resources::ResourceKey::normalize("quiet"),
        data: doc,
        origin_path: None,
        scope: cyrup_resources::ResourceScope::Global,
        origin: cyrup_resources::ResourceOrigin::Builtin,
    };
    let defaults = cyrup_session_svc::TerminalDefaults {
        foreground: Some(cyrup_session_svc::CssColor::from_rgb(1, 2, 3)),
        background: Some(cyrup_session_svc::CssColor::from_rgb(9, 8, 7)),
        appearance: Appearance::Dark,
    };
    let export = cyrup_session_svc::ExportTheme::from_theme_with(&theme, &defaults);
    let text = export.role("text").unwrap();
    let bg = export.role("userMessageBg").unwrap();
    assert_eq!((text.r(), text.g(), text.b()), (1, 2, 3));
    assert_eq!(
        (bg.r(), bg.g(), bg.b()),
        (9, 8, 7),
        "a background token takes the background"
    );
    // Unreported: pi's guess by appearance (`GUESSED_DEFAULT_COLORS`, `theme.ts:216-219`).
    let guessed = cyrup_session_svc::ExportTheme::from_theme(&theme);
    let bg = guessed.role("userMessageBg").unwrap();
    assert_eq!((bg.r(), bg.g(), bg.b()), (0, 0, 0));
}

// ---------------------------------------------------------------------------------------------
// Item 4 — colour-scheme notifications, end to end
// ---------------------------------------------------------------------------------------------

#[cfg(unix)]
mod notifications {
    use std::sync::Arc;
    use std::time::Instant;

    use super::*;
    use crate::color_scheme::{lock_for_test, reset_for_test, set_listener};
    use crate::input::reader::ByteDecoder;
    use crate::terminal_query::{GlobalHub, request_terminal_colors_async_with};

    fn feed(decoder: &mut ByteDecoder, bytes: &[u8]) {
        let mut out = Vec::new();
        decoder.feed(bytes, Instant::now(), &mut out);
        assert!(out.is_empty(), "terminal replies are not typing: {out:?}");
    }

    /// A light/dark report from the terminal re-themes an appearance-following theme and asks the
    /// terminal for its colours again; the answer regenerates the system theme from the new
    /// palette. The whole route is the production one: bytes → `ByteDecoder` → the run loop's
    /// listener → `App::apply_color_scheme_report` → a colour query on the global hub → the reply
    /// bytes → `App::apply_terminal_colors`.
    ///
    /// FAILS without the change: the report was swallowed as an anonymous reply, so the theme never
    /// moved and no query was made.
    #[test]
    fn a_scheme_report_re_themes_the_system_theme_and_requeries_the_terminal() {
        let _guard = lock_for_test();
        reset_for_test();
        let resources = registry();
        let (mut app, _svc, _rx) = wired();
        assert_eq!(app.state().theme.appearance(), Appearance::Dark);

        // The run loop's wiring.
        let (scheme_tx, mut scheme_rx) = tokio::sync::mpsc::unbounded_channel();
        set_listener(Some(Arc::new(move |scheme| {
            let _ = scheme_tx.send(scheme);
        })));
        let mut decoder = ByteDecoder::new(std::time::Duration::from_millis(10));

        // 1. The terminal switches to a light appearance and says so.
        let before = crate::app::requery_count_for_test();
        feed(&mut decoder, b"\x1b[?997;2n");
        let scheme = scheme_rx
            .try_recv()
            .expect("the report reaches the run loop");
        assert_eq!(scheme, TerminalTheme::Light);
        app.apply_color_scheme_report(scheme, &resources);
        assert_eq!(
            app.state().theme.appearance(),
            Appearance::Light,
            "a terminal that reports no background follows the scheme it reported"
        );
        assert_eq!(
            crate::app::requery_count_for_test(),
            before + 1,
            "pi queries the terminal's colours after every report"
        );
        assert_eq!(
            app.theme_controller().unwrap().reported_scheme(),
            Some(TerminalTheme::Light)
        );

        // 2. The query's answer — a light palette — regenerates the theme.
        let (colors_tx, mut colors_rx) = tokio::sync::mpsc::unbounded_channel();
        request_terminal_colors_async_with(
            Vec::new(),
            Arc::new(move |colors| {
                let _ = colors_tx.send(colors);
            }),
            || true,
            &GlobalHub,
        );
        let light = TerminalColors {
            background: Some(Rgb::new(0xef, 0xf1, 0xf5)),
            ..mocha()
        };
        feed(&mut decoder, &terminal_reply(&light));
        let reported = colors_rx
            .try_recv()
            .expect("the late reply completes the query");
        assert_eq!(reported.background, light.background);
        app.apply_terminal_colors(reported, &resources);
        let theme = &app.state().theme;
        assert_eq!(theme.appearance(), Appearance::Light);
        assert!(
            matches!(theme.accent, Some(Color::Rgb(..))),
            "regenerated from the reported palette, not ANSI indices: {:?}",
            theme.accent
        );
        reset_for_test();
    }

    /// Pi ignores a report unless the theme follows the terminal (`autoSyncEnabled`).
    #[test]
    fn a_fixed_theme_ignores_scheme_reports() {
        let _guard = lock_for_test();
        reset_for_test();
        let controller =
            ThemeController::boot(Some("dark"), ColorMode::TrueColor, TerminalTheme::Dark);
        let mut app = App::new(TestBackend::new(80, 24), controller.theme()).unwrap();
        app.set_theme_controller(controller);
        let before = crate::app::requery_count_for_test();
        app.apply_color_scheme_report(TerminalTheme::Light, &registry());
        assert_eq!(app.theme_controller().unwrap().reported_scheme(), None);
        assert_eq!(app.state().theme.name, "dark");
        assert_eq!(
            crate::app::requery_count_for_test(),
            before,
            "no query either"
        );
    }

    /// The controller's own gate (`if (!this.autoSyncEnabled) return`, `theme-controller.ts:241`): a
    /// theme that does not follow the terminal records nothing and re-themes nothing.
    #[test]
    fn a_controller_that_does_not_follow_the_terminal_ignores_a_report() {
        let mut controller =
            ThemeController::boot(Some("dark"), ColorMode::TrueColor, TerminalTheme::Dark);
        assert!(!controller.auto_sync());
        assert_eq!(controller.apply_color_scheme(TerminalTheme::Light), None);
        assert_eq!(controller.reported_scheme(), None);
        assert_eq!(controller.terminal_theme(), TerminalTheme::Dark);
    }

    /// A pair switches to the arm for the reported scheme (`reapplyForTerminal`), and a report that
    /// does not move the appearance leaves the theme alone.
    #[test]
    fn a_pair_switches_arms_on_a_report() {
        let _guard = lock_for_test();
        reset_for_test();
        let controller = ThemeController::boot(
            Some("light/dark"),
            ColorMode::TrueColor,
            TerminalTheme::Dark,
        );
        let mut app = App::new(TestBackend::new(80, 24), controller.theme()).unwrap();
        app.set_theme_controller(controller);
        let resources = registry();
        assert_eq!(app.theme_controller().unwrap().active_name(), "dark");

        app.apply_color_scheme_report(TerminalTheme::Light, &resources);
        assert_eq!(app.theme_controller().unwrap().active_name(), "light");
        assert_eq!(app.state().theme.name, "light");

        app.apply_color_scheme_report(TerminalTheme::Light, &resources);
        assert_eq!(app.theme_controller().unwrap().active_name(), "light");
        app.apply_color_scheme_report(TerminalTheme::Dark, &resources);
        assert_eq!(app.state().theme.name, "dark");
    }

    /// A terminal that reports its background is classified by it; the scheme is the fallback
    /// (`detectTerminalTheme`, `theme.ts:702-710`).
    #[test]
    fn a_reported_background_outranks_the_scheme() {
        let _guard = lock_for_test();
        reset_for_test();
        let mut controller = ThemeController::boot(None, ColorMode::TrueColor, TerminalTheme::Dark);
        controller.apply_terminal_colors(mocha());
        assert_eq!(controller.apply_color_scheme(TerminalTheme::Light), None);
        assert_eq!(controller.terminal_theme(), TerminalTheme::Dark);
    }

    /// Switching the alternate screen back to the inline renderer is pi's `previous.stop()` then
    /// `nextUi.start()` + `rebindTui()` (`interactive-mode.ts:894-920`): the mode goes down with the
    /// screen and comes back up for the renderer that takes over.
    ///
    /// FAILS without the change: nothing re-enabled the mode after the alternate screen's teardown
    /// took it down, so a session that left fullscreen stopped following the terminal's appearance.
    #[test]
    fn leaving_the_alternate_screen_re_arms_the_notifications() {
        use crate::{ModeSwitchOptions, TuiRenderMode};
        let _guard = lock_for_test();
        reset_for_test();
        crate::color_scheme::arm_for_test();
        let mut app = App::new(TestBackend::new(40, 12), UiTheme::dark()).unwrap();
        let _captured = app.enter_fullscreen_captured().expect("renderer builds");
        assert!(
            crate::color_scheme::on_wire_for_test(),
            "the new screen's start turns the mode on"
        );

        let outcome = app.switch_tui_mode(TuiRenderMode::Regular, ModeSwitchOptions::default());
        assert!(outcome.accepted(), "{outcome:?}");
        assert!(
            crate::color_scheme::on_wire_for_test(),
            "the inline renderer is following the terminal again"
        );
        reset_for_test();
    }

    /// A suspend (`Ctrl+Z`) and the external editor release the terminal through `restore()` —
    /// which takes the mode down — and take it back with the same three calls, ending in the
    /// `terminal_started` that brings the mode back (`ui.start()`, `interactive-mode.ts:4371`,
    /// `:4522`). Neither can run without a tty and a foreground job to stop, so the call sites are
    /// pinned the way this crate pins the run loop's: by reading the source they live in.
    #[test]
    fn suspend_and_the_external_editor_restart_the_terminal_with_the_mode() {
        const SRC: &str = include_str!("../app/crossterm.rs");
        let body = |name: &str| {
            let start = SRC.find(name).unwrap_or_else(|| panic!("{name} is gone"));
            let rest = &SRC[start..];
            let end = rest[1..]
                .find("\n    pub fn ")
                .or_else(|| rest[1..].find("\n    fn "))
                .map_or(rest.len(), |e| e + 1);
            rest[..end].to_string()
        };
        for name in ["pub fn suspend(", "fn edit_in_external_editor("] {
            let body = body(name);
            let restart = body
                .find("color_scheme::terminal_started")
                .unwrap_or_else(|| panic!("{name} never restarts the mode"));
            let raw = body
                .find("enable_raw_mode()")
                .expect("it re-enters raw mode");
            assert!(
                raw < restart,
                "{name} restarts the mode after the terminal is back"
            );
        }
        let start = SRC
            .find("pub fn into_stdout(")
            .expect("the production constructor");
        assert!(
            SRC[start..].contains("color_scheme::terminal_started"),
            "the terminal's start is the mode's start"
        );
    }

    /// The mode follows the theme setting: `system` and pairs ask for it, a fixed theme turns it
    /// off, and nothing is written before the terminal has started (the harness).
    #[test]
    fn the_notification_wish_follows_the_theme_setting() {
        let _guard = lock_for_test();
        reset_for_test();
        let controller = ThemeController::boot(None, ColorMode::TrueColor, TerminalTheme::Dark);
        let mut app = App::new(TestBackend::new(80, 24), controller.theme()).unwrap();
        app.set_theme_controller(controller);
        let resources = registry();

        app.settle_boot_theme(None, &resources);
        assert!(
            crate::color_scheme::wanted_for_test(),
            "system follows the terminal"
        );
        app.settle_boot_theme(Some("dark"), &resources);
        assert!(
            !crate::color_scheme::wanted_for_test(),
            "a fixed theme does not"
        );
        app.settle_boot_theme(Some("light/dark"), &resources);
        assert!(crate::color_scheme::wanted_for_test(), "a pair does");
        reset_for_test();
    }
}

// ---------------------------------------------------------------------------------------------
// Item 7 — a token set to `""` is the terminal default
// ---------------------------------------------------------------------------------------------

fn dark_with(token: &str, value: &str) -> UiTheme {
    let mut theme = builtin_themes()
        .into_iter()
        .find(|t| t.data.name == "dark")
        .unwrap();
    theme
        .data
        .colors
        .insert(token.to_string(), cyrup_resources::ColorValue::from(value));
    UiTheme::from_resolved(theme.data.name.clone(), &theme.resolve(), 0)
}

/// `""` is the terminal default, NOT the fallback (`scrollbarTrack ?? muted` falls back only for an
/// absent token: `""` is not nullish), and it survives the colour-depth projection.
#[test]
fn an_empty_token_is_the_terminal_default_not_its_fallback() {
    for mode in [ColorMode::TrueColor, ColorMode::Ansi256] {
        let theme = dark_with("scrollbarTrack", "").with_color_mode(mode);
        assert_eq!(theme.scrollbar_track(), Some(Color::Reset), "{mode:?}");
        assert_ne!(theme.scrollbar_track(), theme.muted);
        let theme = dark_with("scrollbarThumb", "").with_color_mode(mode);
        assert_eq!(theme.scrollbar_thumb(), Some(Color::Reset), "{mode:?}");
        let theme = dark_with("thinkingMax", "").with_color_mode(mode);
        assert_eq!(theme.thinking().max, Color::Reset, "{mode:?}");
        assert_ne!(theme.thinking().max, theme.thinking().xhigh);
    }
}

/// An extension renderer asks the theme for a role through `theme.fg(role, text)`. A role the
/// theme leaves to the terminal is `\x1b[39m` (pi `Theme.addToken`, `theme.ts:290-294`), not the
/// black the bridge used to resolve it to, and a palette index stays an index.
///
/// FAILS without the change: the bridge painted `38;2;0;0;0` for `Color::Reset`, which is
/// invisible on a dark terminal, and flattened palette indices to xterm RGB.
#[test]
fn the_extension_bridge_paints_the_terminal_default_and_palette_indices_as_such() {
    use cyrup_ext::RenderTheme as _;

    // The system theme before the terminal answers: `muted` is the default (dimmed), `accent` a
    // palette slot.
    let mut controller = ThemeController::boot(None, ColorMode::TrueColor, TerminalTheme::Dark);
    let _ = controller.apply_terminal_colors(TerminalColors::default());
    let theme = controller.theme();
    let roles = crate::theme::UiThemeRoles::new(&theme);
    let muted = roles.fg("muted", "x");
    assert!(
        muted.contains("\x1b[39m"),
        "terminal default foreground: {muted:?}"
    );
    assert!(!muted.contains("38;2;0;0;0"), "never black: {muted:?}");
    let accent = roles.fg("accent", "x");
    assert!(
        accent.contains("\x1b[38;5;5m"),
        "palette slot 5: {accent:?}"
    );
}

/// The scrollbar painter writes the terminal default for a `""` track and thumb.
#[test]
fn the_scrollbar_paints_an_empty_token_as_the_terminal_default() {
    use crate::{AltScreen, ScrollbarMode};
    use ratatui::backend::Backend as _;
    let theme = dark_with("scrollbarTrack", "");
    assert_ne!(theme.scrollbar_track(), theme.muted, "not the fallback");
    let (mut alt, _captured) = AltScreen::for_test(TestBackend::new(30, 8), theme).unwrap();
    let rows = 40;
    alt.set_document_for_test(
        (0..rows)
            .map(|i| ratatui::text::Line::from(format!("row {i}")))
            .collect(),
        (0..rows).collect(),
    );
    alt.set_scrollbar_mode(ScrollbarMode::Always);
    alt.draw(None).unwrap();
    let backend = alt.backend_for_test();
    let width = usize::from(backend.size().unwrap().width);
    let track: Vec<_> = backend
        .buffer()
        .content
        .chunks(width)
        .filter_map(|row| row.last())
        .filter(|cell| cell.symbol() == "│")
        .collect();
    assert!(!track.is_empty(), "an overflowing document has a track");
    assert!(
        track.iter().all(|cell| cell.fg == Color::Reset),
        "`scrollbarTrack: \"\"` is the terminal default foreground"
    );
}

// ---------------------------------------------------------------------------------------------
// Item 6 — the `/settings` theme submenu, wired
// ---------------------------------------------------------------------------------------------

mod settings_submenu {
    use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
    use tempfile::TempDir;

    use super::*;
    use crate::color_scheme::{
        lock_for_test, lock_for_test_async, reset_for_test, wanted_for_test,
    };
    use crate::tests::harness::{buf_text, key};
    use crate::{AppAction, AppCommand, SelectorKind, SettingRow, SettingsSelector};
    use ratatui::crossterm::event::KeyCode;

    async fn session() -> (TempDir, Arc<AgentSession>) {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let mut cfg = SessionConfig::new(cwd, agent_dir);
        cfg.trust_override = Some(true);
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        (
            tmp,
            Arc::new(SessionBuilder::new(provider, cfg).build().await.unwrap()),
        )
    }

    /// An app on the system theme with `/settings` open on its Theme row.
    fn settings_app(setting: Option<&str>) -> App<TestBackend> {
        let controller = ThemeController::boot(setting, ColorMode::TrueColor, TerminalTheme::Dark);
        let mut app = App::new(TestBackend::new(80, 30), controller.theme()).unwrap();
        let selection = controller.theme_selection().to_string();
        app.set_theme_controller(controller);
        app.open_boxed_selector(
            SelectorKind::Settings,
            Box::new(SettingsSelector::new(
                "Settings",
                vec![SettingRow::submenu("theme", "Theme", selection, "theme")],
            )),
        );
        app
    }

    fn press(app: &mut App<TestBackend>, code: KeyCode) -> AppAction {
        app.handle_input(&key(code))
    }

    /// The Theme row opens pi's `ThemeSubmenu`, and building a pair from its two pickers previews
    /// it live (the half the terminal's appearance selects), then confirming persists `light/dark`,
    /// re-queries the terminal's colours (`onThemeChange` → `applyFromSettings`) and arms the
    /// appearance notifications; picking a fixed theme afterwards disarms them.
    ///
    /// FAILS without the change: the Theme row opened a flat list of the two built-ins — no
    /// `automatic`, no pair, no custom themes — and a `light/dark` value could not be produced.
    #[tokio::test]
    async fn a_pair_is_built_previewed_confirmed_and_arms_the_notifications() {
        let _guard = lock_for_test_async().await;
        reset_for_test();
        let (_tmp, session) = session().await;
        let mut app = settings_app(Some("dark"));

        // Enter on the Theme row.
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.active_selector_kind(), Some(SelectorKind::Theme));
        app.draw().unwrap();
        let text = buf_text(&app);
        assert!(
            text.contains("automatic"),
            "pi's single menu offers `automatic`:\n{text}"
        );
        assert!(text.contains("✓ dark"), "{text}");

        // `dark` is highlighted; `automatic` is above it (system, automatic, dark, light).
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        app.draw().unwrap();
        assert!(buf_text(&app).contains("Automatic Theme"));

        // Light theme: the picker opens on `dark`; one step down is `light`.
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        assert_eq!(
            app.state().theme.name,
            "light",
            "moving in the picker previews the theme under the highlight"
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.state().theme.name,
            "dark",
            "`light/dark` on a dark terminal previews its dark half"
        );
        // Dark theme (the row below): its picker opens on `dark`, which stays.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);

        // Apply.
        press(&mut app, KeyCode::Down);
        let before = crate::app::requery_count_for_test();
        let action = press(&mut app, KeyCode::Enter);
        let AppAction::Command(AppCommand::ApplySetting { id, value }) = action else {
            panic!("applying the pair persists it, got {action:?}");
        };
        assert_eq!((id.as_str(), value.as_str()), ("theme", "light/dark"));
        assert_eq!(
            crate::app::requery_count_for_test(),
            before + 1,
            "`onThemeChange` ends in `applyFromSettings`, which asks the terminal for its colours"
        );
        assert_eq!(
            app.active_selector_kind(),
            Some(SelectorKind::Settings),
            "back on /settings"
        );

        // The persist arm records the setting and arms the notifications.
        app.execute_command(
            AppCommand::ApplySetting {
                id,
                value: value.clone(),
            },
            &session,
            None,
        )
        .await;
        assert!(
            wanted_for_test(),
            "a pair follows the terminal's appearance"
        );
        assert_eq!(
            app.theme_controller().unwrap().current_setting(),
            Some("light/dark")
        );
        // …and the Theme row now shows the pair.
        app.draw().unwrap();
        assert!(buf_text(&app).contains("light/dark"));

        // A fixed theme turns them off again.
        app.execute_command(
            AppCommand::ApplySetting {
                id: "theme".to_string(),
                value: "light".to_string(),
            },
            &session,
            None,
        )
        .await;
        assert!(!wanted_for_test());
        reset_for_test();
    }

    /// `AgentSession::export_theme` — the call `/export` and `/share` make — takes the palette the
    /// TUI holds for the active theme: the generated `system` theme carries the terminal's colours
    /// into the page. pi's `getResolvedThemeColors()` reads the CURRENT theme's colours
    /// (`theme.ts:903-906` @v1.0.0).
    ///
    /// FAILS without the change: `export_theme` looked the name `system` up in the resources, found
    /// nothing and exported the compiled-in `dark` palette.
    #[tokio::test]
    async fn the_session_export_of_the_system_theme_carries_the_generated_palette() {
        let _guard = lock_for_test_async().await;
        reset_for_test();
        let (_tmp, session) = session().await;
        let controller = ThemeController::boot(None, ColorMode::TrueColor, TerminalTheme::Dark);
        let mut app = App::new(TestBackend::new(80, 30), controller.theme()).unwrap();
        app.set_theme_controller(controller);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.install_extension_readbacks(&session.services().host_services, registry(), tx);
        app.apply_terminal_colors(mocha(), &registry());
        app.draw().unwrap();

        let Some(Color::Rgb(r, g, b)) = app.state().theme.accent else {
            panic!("generated accent");
        };
        let html = session.export_html_document().await.unwrap();
        assert!(
            html.contains(&format!("--accent: #{r:02x}{g:02x}{b:02x};")),
            "the page carries the generated accent"
        );

        // A fixed theme exports as itself, `""` tokens filled from the terminal.
        app.apply_theme_switch(ThemeSwitch::Resource(
            registry().themes.get_name("light").unwrap().clone(),
        ));
        app.draw().unwrap();
        let light = session.export_html_document().await.unwrap();
        assert!(
            light.contains("--body-bg: #efeeee;"),
            "light's own export block"
        );
        reset_for_test();
    }

    /// Esc out of the submenu restores the theme it opened on, whatever it previewed.
    #[test]
    fn escape_restores_the_theme_the_submenu_opened_on() {
        let _guard = lock_for_test();
        reset_for_test();
        let mut app = settings_app(Some("dark"));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.state().theme.name, "light", "previewed");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.state().theme.name, "dark");
        assert_eq!(app.active_selector_kind(), Some(SelectorKind::Settings));
    }

    /// The submenu offers every theme the session discovered — a user's own as well as the
    /// built-ins — and previews a file theme from its document.
    #[test]
    fn a_custom_theme_is_offered_and_previewed() {
        let _guard = lock_for_test();
        reset_for_test();
        let doc: cyrup_resources::theme::ThemeData = serde_json::from_str(
            r##"{"name":"quiet","colors":{"text":"#112233","accent":"#445566"}}"##,
        )
        .unwrap();
        let custom = cyrup_resources::Theme {
            key: cyrup_resources::ResourceKey::normalize("quiet"),
            data: doc,
            origin_path: Some(std::path::PathBuf::from("/themes/quiet.json")),
            scope: cyrup_resources::ResourceScope::Global,
            origin: cyrup_resources::ResourceOrigin::Builtin,
        };
        let mut all = builtin_themes();
        all.push(custom);
        let resources = Arc::new(ResourceRegistry {
            themes: ResourceSet::build(all),
            ..ResourceRegistry::default()
        });
        let mut app = settings_app(Some("dark"));
        let svc = services();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.install_extension_readbacks(&svc, resources, tx);

        press(&mut app, KeyCode::Enter);
        app.draw().unwrap();
        assert!(buf_text(&app).contains("quiet"), "{}", buf_text(&app));
        // dark -> light -> quiet
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.state().theme.name, "quiet");
        assert_eq!(app.state().theme.accent, Some(Color::Rgb(0x44, 0x55, 0x66)));
        reset_for_test();
    }
}
