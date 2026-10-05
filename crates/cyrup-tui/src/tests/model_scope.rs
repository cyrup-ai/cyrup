//! The `Model scope:` startup line — pi `init()` (`interactive-mode.ts:940-953` @v1.0.0):
//! `session.scopedModels.length > 0 && shouldShowStartupDetails()` prints
//! `theme.fg("dim", "Model scope: <id[:thinking], …>" + theme.fg("muted", " (<Keys> to cycle)"))`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_config::settings::QuietStartup;
use cyrup_core::{ModelId, ModelThinkingLevel};
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::ScopedModel;
use ratatui::backend::TestBackend;

use crate::{App, ModelScopeBanner, UiTheme, cycle_forward_keys};

fn scoped(id: &str, level: Option<ModelThinkingLevel>) -> ScopedModel {
    let mut model = FauxProvider::new().model().clone();
    model.id = ModelId::from(id);
    ScopedModel {
        model,
        thinking_level: level,
    }
}

fn banner(
    scope: &[ScopedModel],
    quiet: QuietStartup,
    verbose: bool,
    keys: Option<&str>,
) -> Option<ModelScopeBanner> {
    ModelScopeBanner::for_startup(scope, quiet, verbose, keys, &UiTheme::dark())
}

fn text(scope: &[ScopedModel]) -> String {
    banner(scope, QuietStartup::Off, false, Some("ctrl+p"))
        .expect("shown")
        .text()
}

#[test]
fn no_scoped_models_prints_nothing_even_when_details_are_shown() {
    assert_eq!(banner(&[], QuietStartup::Off, true, Some("ctrl+p")), None);
}

#[test]
fn one_scoped_model_without_a_thinking_level() {
    assert_eq!(
        text(&[scoped("sonnet", None)]),
        "Model scope: sonnet (Ctrl+P to cycle)"
    );
}

#[test]
fn one_scoped_model_with_a_thinking_level() {
    assert_eq!(
        text(&[scoped("sonnet", Some(ModelThinkingLevel::High))]),
        "Model scope: sonnet:high (Ctrl+P to cycle)"
    );
}

#[test]
fn many_scoped_models_are_comma_joined_and_only_the_set_levels_get_a_suffix() {
    // `off` is a set level in pi (a non-empty string), so it is printed.
    assert_eq!(
        text(&[
            scoped("a", Some(ModelThinkingLevel::Xhigh)),
            scoped("b", None),
            scoped("c", Some(ModelThinkingLevel::Off)),
            scoped("d", Some(ModelThinkingLevel::Minimal)),
        ]),
        "Model scope: a:xhigh, b, c:off, d:minimal (Ctrl+P to cycle)"
    );
}

#[test]
fn the_line_is_dim_with_a_muted_hint() {
    let theme = UiTheme::dark();
    let b = banner(
        &[scoped("a", None)],
        QuietStartup::Off,
        false,
        Some("ctrl+p"),
    )
    .unwrap();
    let spans = &b.line().spans;
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content, "Model scope: a");
    assert_eq!(spans[0].style, theme.dim_style());
    assert_eq!(spans[1].content, " (Ctrl+P to cycle)");
    assert_eq!(spans[1].style, theme.muted_style());
    assert_ne!(theme.dim_style(), theme.muted_style());
}

#[test]
fn an_unbound_cycle_key_prints_the_line_without_the_hint() {
    let b = banner(&[scoped("a", None)], QuietStartup::Off, false, None).unwrap();
    assert_eq!(b.text(), "Model scope: a");
    assert_eq!(b.line().spans.len(), 1);
}

#[test]
fn quiet_startup_gates_the_line_and_verbose_overrides_it() {
    let scope = [scoped("a", None)];
    let shown = |q, v| banner(&scope, q, v, Some("ctrl+p")).is_some();
    // `shouldShowStartupDetails`: `verbose || quietStartup === false`.
    assert!(shown(QuietStartup::Off, false));
    assert!(!shown(QuietStartup::On, false));
    assert!(!shown(QuietStartup::Header, false));
    assert!(shown(QuietStartup::On, true));
    assert!(shown(QuietStartup::Header, true));
    assert!(shown(QuietStartup::Off, true));
}

#[test]
fn the_cycle_key_is_the_users_resolved_binding() {
    assert_eq!(cycle_forward_keys(None).as_deref(), Some("ctrl+p"));
    assert_eq!(cycle_forward_keys(Some("{}")).as_deref(), Some("ctrl+p"));

    let rebound = cycle_forward_keys(Some(r#"{"app.model.cycleForward": "ctrl+g"}"#));
    assert_eq!(rebound.as_deref(), Some("ctrl+g"));
    let line = banner(
        &[scoped("a", None)],
        QuietStartup::Off,
        false,
        rebound.as_deref(),
    )
    .unwrap()
    .text();
    assert_eq!(line, "Model scope: a (Ctrl+G to cycle)");

    // Several keys are all named, `/`-joined (`formatKeyText(cycleKeys.join("/"))`).
    let two = cycle_forward_keys(Some(r#"{"app.model.cycleForward": ["ctrl+g", "ctrl+h"]}"#));
    assert_eq!(two.as_deref(), Some("ctrl+g/ctrl+h"));
    let line = banner(
        &[scoped("a", None)],
        QuietStartup::Off,
        false,
        two.as_deref(),
    )
    .unwrap()
    .text();
    assert_eq!(line, "Model scope: a (Ctrl+G/Ctrl+H to cycle)");

    // Unbound: no hint at all.
    assert_eq!(
        cycle_forward_keys(Some(r#"{"app.model.cycleForward": []}"#)),
        None
    );
}

#[test]
fn a_rejected_keybindings_document_leaves_the_default_key() {
    assert_eq!(
        cycle_forward_keys(Some("not json")).as_deref(),
        Some("ctrl+p")
    );
}

#[test]
fn the_written_bytes_are_the_styled_line_and_a_newline_and_no_screen_control() {
    let b = banner(
        &[scoped("a", None)],
        QuietStartup::Off,
        false,
        Some("ctrl+p"),
    )
    .unwrap();
    let mut out = Vec::new();
    b.write_to(&mut out).unwrap();
    let written = String::from_utf8(out).unwrap();
    assert!(written.contains("Model scope: a"), "{written:?}");
    assert!(written.contains(" (Ctrl+P to cycle)"), "{written:?}");
    assert!(written.ends_with("\n"), "{written:?}");
    assert!(!written.ends_with("\r\n"), "{written:?}");
    // Styled (an SGR is in there), but never an alternate-screen, erase or cursor-move sequence:
    // it is a scrollback line, not a frame.
    assert!(written.contains("\x1b["), "{written:?}");
    for control in ["\x1b[?1049", "\x1b[2J", "\x1b[J", "\x1b[H"] {
        assert!(!written.contains(control), "{control:?} in {written:?}");
    }
    // The styling is closed again before the newline, so it cannot bleed into what follows.
    assert!(written.trim_end_matches('\n').ends_with("m"), "{written:?}");
}

/// Fullscreen: pi's `console.log` precedes the alternate screen, so the line is on the main screen
/// and the alternate screen never draws it. The banner goes to the pre-mount stream; entering
/// fullscreen and painting frames adds nothing of it to the alternate screen, and the bytes that
/// enter it carry no copy either.
#[test]
fn the_fullscreen_alternate_screen_does_not_draw_the_line() {
    let mut premount = Vec::new();
    banner(
        &[scoped("zzq-scoped", None)],
        QuietStartup::Off,
        false,
        Some("ctrl+p"),
    )
    .unwrap()
    .write_to(&mut premount)
    .unwrap();
    assert!(String::from_utf8_lossy(&premount).contains("zzq-scoped"));

    let mut app = App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap();
    let captured = app.enter_fullscreen_captured().expect("renderer builds");
    app.draw().unwrap();
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    let buf = alt.backend_for_test().buffer().clone();
    let screen: String = (0..24)
        .flat_map(|y| (0..100).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect();
    assert!(!screen.contains("Model scope"), "{screen}");
    assert!(!screen.contains("zzq-scoped"), "{screen}");
    drop(captured);
}
