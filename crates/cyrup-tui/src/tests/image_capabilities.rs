//! Terminal image-capability env-sniff (feature #7; Pi `detectCapabilities`, terminal-image.ts:65).
//!
//! The old probe sent an APC round-trip to the TTY (`Picker::from_query_stdio`) and blocked reading
//! its reply. The env-sniff below matches Pi's `detectCapabilities` ordering exactly — multiplexer
//! suppression first, then positively-identified terminals, then a conservative default — and never
//! touches stdin. `detect_capabilities_from` is the pure core, parameterised over an env lookup + the
//! tmux-forwards-hyperlinks flag, so both branches are deterministic here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{detect_capabilities_from, detect_capabilities_on_platform, ImageProtocol};

/// Build an env lookup from a fixed table (missing keys ⇒ `None`, exactly like `std::env::var`).
fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k: &str| pairs.iter().find(|(key, _)| *key == k).map(|(_, v)| v.to_string())
}

#[test]
fn kitty_family_negotiates_the_kitty_protocol() {
    for env in [
        vec![("KITTY_WINDOW_ID", "1")],
        vec![("TERM_PROGRAM", "kitty")],
        vec![("TERM_PROGRAM", "ghostty")],
        vec![("WEZTERM_PANE", "0")],
        vec![("TERM_PROGRAM", "WarpTerminal")],
    ] {
        let caps = detect_capabilities_from(env_of(&env), false);
        assert_eq!(caps.images, Some(ImageProtocol::Kitty), "env {env:?} → kitty");
        assert!(caps.hyperlinks, "identified terminals forward hyperlinks");
        assert!(caps.true_color);
    }
}

#[test]
fn iterm2_negotiates_iterm2() {
    let caps = detect_capabilities_from(env_of(&[("ITERM_SESSION_ID", "w0")]), false);
    assert_eq!(caps.images, Some(ImageProtocol::Iterm2));
    let caps2 = detect_capabilities_from(env_of(&[("TERM_PROGRAM", "iTerm.app")]), false);
    assert_eq!(caps2.images, Some(ImageProtocol::Iterm2));
}

#[test]
fn tmux_and_screen_suppress_images_and_gate_hyperlinks() {
    // tmux: images are unreliable (None); OSC-8 only when tmux confirms forwarding.
    let tmux_no = detect_capabilities_from(env_of(&[("TMUX", "/tmp/tmux-1000/default,1,0")]), false);
    assert_eq!(tmux_no.images, None);
    assert!(!tmux_no.hyperlinks, "tmux without forwarding must not emit OSC-8");
    let tmux_yes = detect_capabilities_from(env_of(&[("TMUX", "x")]), true);
    assert!(tmux_yes.hyperlinks, "tmux with forwarding enables OSC-8");
    assert_eq!(tmux_yes.images, None);
    // `TERM=tmux-256color` is the multiplexer too (no `TMUX` var), still suppressed.
    let tmux_term = detect_capabilities_from(env_of(&[("TERM", "tmux-256color")]), true);
    assert_eq!(tmux_term.images, None);

    // screen never forwards OSC-8.
    let screen = detect_capabilities_from(env_of(&[("TERM", "screen-256color")]), true);
    assert_eq!(screen.images, None);
    assert!(!screen.hyperlinks, "screen never forwards OSC-8");
}

#[test]
fn known_no_image_terminals_and_conservative_default() {
    // Windows Terminal / VSCode / Alacritty: truecolor + hyperlinks, but no inline images.
    for tp in ["vscode", "alacritty"] {
        let caps = detect_capabilities_from(env_of(&[("TERM_PROGRAM", tp)]), false);
        assert_eq!(caps.images, None, "{tp} has no inline image protocol");
        assert!(caps.hyperlinks);
    }
    let wt = detect_capabilities_from(env_of(&[("WT_SESSION", "guid")]), false);
    assert_eq!(wt.images, None);
    assert!(wt.hyperlinks);

    // JetBrains JediTerm: no images, no hyperlinks.
    let jb = detect_capabilities_from(env_of(&[("TERMINAL_EMULATOR", "JetBrains-JediTerm")]), false);
    assert_eq!(jb.images, None);
    assert!(!jb.hyperlinks);

    // Unknown terminal: conservative — no images, OSC-8 off, truecolor only if COLORTERM hinted it.
    let unknown = detect_capabilities_from(env_of(&[("TERM", "xterm-256color")]), false);
    assert_eq!(unknown.images, None);
    assert!(!unknown.hyperlinks, "unidentified terminals default OSC-8 off (Pi's legacy `text (url)`)");
    assert!(!unknown.true_color, "no COLORTERM hint → truecolor off");
    let unknown_tc = detect_capabilities_from(
        env_of(&[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]),
        false,
    );
    assert!(unknown_tc.true_color, "COLORTERM=truecolor → truecolor on");
}

/// Pi v0.84.1 `tui/src/terminal-image.ts:124-129` — a Windows console that set no `WT_SESSION`
/// (Windows Terminal hosting a `cmd.exe` launched straight from Win+R) still gets truecolor, and
/// still does not get OSC-8. Added upstream in `fa07e7bd9`, after cyrup's v0.83.0 baseline.
///
/// The platform is a compile-time `cfg!(windows)` in `detect_capabilities_from`, so this drives the
/// decision function directly with a synthesized `is_windows_console` instead of the real platform.
#[test]
fn a_bare_windows_console_assumes_truecolor_without_wt_session() {
    // No COLORTERM hint, no identified terminal — on Windows this is still truecolor.
    let win = detect_capabilities_on_platform(env_of(&[("TERM", "xterm-256color")]), false, true);
    assert!(win.true_color, "terminal-image.ts:128 — modern Windows consoles support truecolor");
    assert!(!win.hyperlinks, "terminal-image.ts:126 — hyperlinks stay off unless positively detected");
    assert_eq!(win.images, None, "terminal-image.ts:128 — `images: null`");

    // Even with no env at all.
    let bare = detect_capabilities_on_platform(env_of(&[]), false, true);
    assert!(bare.true_color);
    assert!(!bare.hyperlinks);

    // MIRROR 1: the identical environment on a non-Windows platform is unchanged — conservative.
    let unix = detect_capabilities_on_platform(env_of(&[("TERM", "xterm-256color")]), false, false);
    assert!(!unix.true_color, "terminal-image.ts:131 — no COLORTERM hint off Windows ⇒ truecolor off");
    assert!(!unix.hyperlinks);

    // MIRROR 2: the Windows branch sits AFTER every positive identification (terminal-image.ts:124),
    // so it must not steal a terminal that was already identified.
    let wt = detect_capabilities_on_platform(env_of(&[("WT_SESSION", "guid")]), false, true);
    assert!(wt.hyperlinks, "WT_SESSION (`:108-110`) still wins over the bare-console fallback");
    let kitty = detect_capabilities_on_platform(env_of(&[("KITTY_WINDOW_ID", "1")]), false, true);
    assert_eq!(kitty.images, Some(ImageProtocol::Kitty));
    assert!(kitty.hyperlinks);
    let jb = detect_capabilities_on_platform(
        env_of(&[("TERMINAL_EMULATOR", "JetBrains-JediTerm")]),
        false,
        true,
    );
    assert!(!jb.hyperlinks);
    assert!(jb.true_color);

    // MIRROR 3: the multiplexer branches precede it (`:76-86`), so tmux on Windows keeps its own
    // `hasTrueColorHint`-derived truecolor rather than the Windows assumption.
    let tmux = detect_capabilities_on_platform(env_of(&[("TMUX", "x")]), false, true);
    assert!(!tmux.true_color, "terminal-image.ts:80 — tmux uses hasTrueColorHint, not the win32 rule");
    let screen = detect_capabilities_on_platform(env_of(&[("TERM", "screen-256color")]), false, true);
    assert!(!screen.true_color, "terminal-image.ts:85 — screen likewise");
}

/// Pi v0.84.1 `tui/src/terminal-image.ts:73`:
/// `const hasTrueColorHint = colorTerm === "truecolor" || colorTerm === "24bit";`
/// — an equality, not a substring test. Port bug: this was `contains` in cyrup, and was equally
/// wrong at v0.83.0, where the same line reads identically.
///
/// HOST-INDEPENDENCE: this drives [`detect_capabilities_on_platform`] with `is_windows_console`
/// passed explicitly, NOT the [`detect_capabilities_from`] wrapper that hard-wires `cfg!(windows)`.
/// The wrapper would make the negative assertions host-dependent: on a Windows host the
/// `isWindowsConsole` branch (`image.rs:510`, Pi `:124-129`) returns `true_color: true` before the
/// conservative default at `:131` is ever reached, so `assert!(!hint("not-truecolor"))` would fail
/// there for a reason that has nothing to do with the equality-vs-substring property under test.
/// Parameterising pins the property on every host.
///
/// NOT COVERED HERE: passing `is_windows_console = true` simulates the platform flag on whatever
/// host runs the suite — it is NOT coverage of a real Windows console. Nothing in this file has
/// ever been executed on a Windows host from this workspace; every assertion about the win32 branch
/// (here and in [`a_bare_windows_console_assumes_truecolor_without_wt_session`]) tests only that
/// cyrup's port takes Pi's `:124-129` path when told the platform is win32, not that Windows
/// terminals actually behave that way.
#[test]
fn the_colorterm_hint_is_an_equality_not_a_substring() {
    // LINUX PATH (`is_windows_console = false`) — reaches Pi's conservative default at `:131`,
    // so `true_color` is exactly `hasTrueColorHint`.
    let hint =
        |v: &str| detect_capabilities_on_platform(env_of(&[("COLORTERM", v)]), false, false).true_color;
    assert!(hint("truecolor"));
    assert!(hint("24bit"));
    assert!(hint("TrueColor"), "Pi lowercases COLORTERM first (`:72`)");
    // Values that merely EMBED the token are not a hint.
    assert!(!hint("not-truecolor"));
    assert!(!hint("truecolor-maybe"));
    assert!(!hint("24bitish"));
    assert!(!hint("gnome-terminal"));
    assert!(!hint(""));

    // WINDOWS PATH (`is_windows_console = true`) — Pi `:124-129` returns before the default, so
    // truecolor is asserted regardless of COLORTERM. Asserted only to document that the equality
    // fix does NOT change this branch; it is not a substring-vs-equality observation.
    for v in ["not-truecolor", "truecolor", ""] {
        let caps = detect_capabilities_on_platform(env_of(&[("COLORTERM", v)]), false, true);
        assert!(caps.true_color, "terminal-image.ts:124-129 — the win32 branch assumes truecolor");
    }
}

/// TUI-N12 — the capability cache is now settable AND resettable, over the whole record.
///
/// RED at HEAD: the only counterpart was `seed_hyperlink_support`, a first-writer-wins
/// `OnceLock::set` that (a) could not be overwritten or reset once read and (b) carried only the
/// `hyperlinks` field. Because there was no way to pin the global, a test wanting the non-ambient
/// branch had to use a per-call override, and that override existed for exactly one consumer
/// (`render_with_hyperlink_support`) — which is the structural hole TUI-N11 fell through.
///
/// Pi exports both mutators alongside the getter: `resetCapabilitiesCache()`
/// (`packages/tui/src/terminal-image.ts:137-139`) and `setCapabilities(caps)` (`:142-144`,
/// doc-commented "Override the cached capabilities. Useful in tests to exercise both code paths").
///
/// Serialized with the other cache-touching test in this file because the cache is process-wide.
#[test]
fn set_capabilities_pins_both_branches_and_reset_drops_the_pin() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    crate::set_capabilities(crate::TerminalCapabilities {
        images: Some(crate::ImageProtocol::Kitty),
        true_color: true,
        hyperlinks: true,
    });
    assert!(crate::hyperlinks_supported(), "the pinned `true` branch must be reachable");
    assert_eq!(crate::cached_capabilities().images, Some(crate::ImageProtocol::Kitty));

    // …and the OTHER branch, which the old write-once lock made unreachable in the same process.
    crate::set_capabilities(crate::TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: false,
    });
    assert!(!crate::hyperlinks_supported(), "the pinned `false` branch must be reachable too");

    crate::reset_capabilities_cache();
    // After a reset the next read re-detects from the ambient environment; assert only that it does
    // not keep the pin, never what the ambient answer IS (that is the TUI-N11 mistake).
    let _ = crate::cached_capabilities();
    crate::reset_capabilities_cache();
}

/// The process-wide cache makes these tests order-dependent; one lock serializes them.
static CAPS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
