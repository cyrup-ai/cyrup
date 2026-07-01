//! Terminal image-capability env-sniff (feature #7; Pi `detectCapabilities`, terminal-image.ts:65).
//!
//! The old probe sent an APC round-trip to the TTY (`Picker::from_query_stdio`) and blocked reading
//! its reply. The env-sniff below matches Pi's `detectCapabilities` ordering exactly — multiplexer
//! suppression first, then positively-identified terminals, then a conservative default — and never
//! touches stdin. `detect_capabilities_from` is the pure core, parameterised over an env lookup + the
//! tmux-forwards-hyperlinks flag, so both branches are deterministic here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::{detect_capabilities_from, ImageProtocol};

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
