#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
//! Presentation-fidelity guards for `cyrup-tui`'s style construction, T1–T9 of `docs/audits/2026-08-09-tui-presentation-fidelity.md` §2.
//!
//! Every assertion here is anchored to a line of pi **v0.84.1** that was read, not inferred. The
//! shared theme file is where a single wrong accessor reaches dozens of render sites, so these pin
//! the *resolved* colour and the *exact* SGR attribute set rather than comparing one accessor to
//! another (a comparison that tracks the bug when the bug is in the accessor).
//!
//! Upstream paths, all under `pi/packages/`:
//!   - `coding-agent/src/modes/interactive/theme/theme.ts` — `Theme.fg`/`bold`, the token list,
//!     `getThinkingBorderColor`, `buildCliHighlightTheme`, `getMarkdownTheme`, `getEditorTheme`.
//!   - `coding-agent/src/modes/interactive/theme/{dark,light}.json` — the token values.
//!   - `tui/src/terminal-image.ts` — `detectCapabilities`.
//!   - `tui/src/components/markdown.ts` — code-block and link emission.

use crate::{ColorMode, UiTheme, render_markdown};
use ratatui::style::{Color, Modifier, Style};

fn env_of(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| owned.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone())
}

// ---------------------------------------------------------------------------------------------
// T1 — `dim_style()` must resolve the `dim` TOKEN, colour only.
// ---------------------------------------------------------------------------------------------

/// Pi renders every hint through `theme.fg("dim", …)` (e.g. `theme.ts:1312`, `:1314`), and `fg()`
/// (`theme.ts:372-376`) emits a bare foreground escape terminated by `\x1b[39m` — **no** SGR
/// attribute. The token is `dimGray`: `#666666` in `dark.json:31`+`:12`, `#767676` in
/// `light.json:30`+`:12`.
///
/// FAILS before the fix on both counts: the accessor resolved `text` (`#d4d4d4` dark / `#1f2328`
/// light) and added `Modifier::DIM`.
#[test]
fn t1_dim_style_resolves_the_dim_token_with_no_sgr_attribute() {
    let dark = UiTheme::dark().dim_style();
    assert_eq!(
        dark.fg,
        Some(Color::Rgb(0x66, 0x66, 0x66)),
        "dark `dim` is dimGray #666666"
    );
    assert!(
        !dark.add_modifier.contains(Modifier::DIM),
        "pi's fg() emits no SGR 2; terminals that drop it rendered hints at full body brightness"
    );
    assert_eq!(
        dark.add_modifier,
        Modifier::empty(),
        "colour only, like Theme.fg"
    );

    let light = UiTheme::light().dim_style();
    assert_eq!(
        light.fg,
        Some(Color::Rgb(0x76, 0x76, 0x76)),
        "light `dim` is dimGray #767676"
    );
    assert_eq!(light.add_modifier, Modifier::empty());

    // The old wrong answers, named so a revert cannot pass by accident.
    assert_ne!(
        dark.fg,
        UiTheme::dark().base_style().fg,
        "`dim` is not the `text` role"
    );
    assert_ne!(light.fg, UiTheme::light().base_style().fg);
}

/// MIRROR: `muted` is a *different* token and must not have moved. `dark.json:30 "muted": "gray"`
/// = `#808080`, and `theme.ts:1291-1295` uses it for select-list descriptions/scroll info.
#[test]
fn t1_mirror_muted_token_is_untouched_and_distinct_from_dim() {
    let t = UiTheme::dark();
    assert_eq!(t.muted_style().fg, Some(Color::Rgb(0x80, 0x80, 0x80)));
    assert_eq!(t.muted_style().add_modifier, Modifier::empty());
    assert_ne!(
        t.muted_style().fg,
        t.dim_style().fg,
        "muted #808080 vs dim #666666"
    );
}

// ---------------------------------------------------------------------------------------------
// T4 — `error_style()` must not bake in bold.
// ---------------------------------------------------------------------------------------------

/// `Theme.fg` is colour-only and `Theme.bold` is a separate combinator (`theme.ts:372-376` vs
/// `:384-386`). `git grep -c 'bold(theme.fg("error"' v0.84.1 -- packages` matches nothing: no
/// upstream error string is bold. FAILS before the fix, which added `Modifier::BOLD`.
#[test]
fn t4_error_style_is_colour_only() {
    let t = UiTheme::dark();
    let e = t.error_style();
    assert_eq!(
        e.fg,
        Some(Color::Rgb(0xcc, 0x66, 0x66)),
        "dark `error` is red #cc6666"
    );
    assert!(
        !e.add_modifier.contains(Modifier::BOLD),
        "pi renders every error string unbolded — see assistant-message.ts / bash-execution.ts"
    );
    assert_eq!(e.add_modifier, Modifier::empty());
}

/// MIRROR: bold where pi really does compose `bold()`. Tool titles are
/// `theme.fg("toolTitle", theme.bold("read"))` (`core/tools/read.ts:81`), and the user label is
/// bold accent; neither may lose its weight to the T4 fix.
#[test]
fn t4_mirror_styles_pi_really_bolds_stay_bold() {
    let t = UiTheme::dark();
    assert!(t.tool_title_style().add_modifier.contains(Modifier::BOLD));
    assert!(t.user_style().add_modifier.contains(Modifier::BOLD));
    assert!(t.md_heading_style().add_modifier.contains(Modifier::BOLD));
}

// ---------------------------------------------------------------------------------------------
// T2 / T3 — colour-mode detection.
// ---------------------------------------------------------------------------------------------

/// Pi gates the mode on `getCapabilities().trueColor` (`theme.ts:611`), and `detectCapabilities`
/// (`tui/src/terminal-image.ts:68-132`) returns `trueColor: true` for a whole table of terminal
/// programs. None of these set `COLORTERM`, so before the fix every one of them was quantised
/// through the 6×6×6 cube.
#[test]
fn t2_identified_terminals_get_truecolor_without_colorterm() {
    // iTerm2 — terminal-image.ts:104-106 (ITERM_SESSION_ID / TERM_PROGRAM=iterm.app).
    assert_eq!(
        ColorMode::detect_from(env_of(&[("ITERM_SESSION_ID", "w0t0")])),
        ColorMode::TrueColor
    );
    assert_eq!(
        ColorMode::detect_from(env_of(&[("TERM_PROGRAM", "iTerm.app")])),
        ColorMode::TrueColor
    );
    // Windows Terminal — terminal-image.ts:108-110 (WT_SESSION).
    assert_eq!(
        ColorMode::detect_from(env_of(&[("WT_SESSION", "guid")])),
        ColorMode::TrueColor
    );
    // vscode / alacritty — terminal-image.ts:112-118.
    assert_eq!(
        ColorMode::detect_from(env_of(&[("TERM_PROGRAM", "vscode")])),
        ColorMode::TrueColor
    );
    assert_eq!(
        ColorMode::detect_from(env_of(&[("TERM_PROGRAM", "alacritty")])),
        ColorMode::TrueColor
    );
    // JetBrains JediTerm — terminal-image.ts:120-122.
    assert_eq!(
        ColorMode::detect_from(env_of(&[("TERMINAL_EMULATOR", "JetBrains-JediTerm")])),
        ColorMode::TrueColor
    );
    // kitty / ghostty / wezterm / warp — terminal-image.ts:88-102.
    for (k, v) in [
        ("KITTY_WINDOW_ID", "1"),
        ("GHOSTTY_RESOURCES_DIR", "/x"),
        ("WEZTERM_PANE", "0"),
        ("WARP_SESSION_ID", "s"),
    ] {
        assert_eq!(
            ColorMode::detect_from(env_of(&[(k, v)])),
            ColorMode::TrueColor,
            "{k}"
        );
    }
}

/// MIRROR: `COLORTERM` remains the fallback hint for an *unidentified* terminal, matched by strict
/// equality (`terminal-image.ts:73` `colorTerm === "truecolor" || colorTerm === "24bit"`), and an
/// unidentified terminal without it stays on 256 colours (`:131` → `theme.ts:611`).
#[test]
fn t2_mirror_colorterm_is_still_the_fallback_hint_for_unknown_terminals() {
    assert_eq!(
        ColorMode::detect_from(env_of(&[
            ("TERM", "xterm-256color"),
            ("COLORTERM", "truecolor")
        ])),
        ColorMode::TrueColor
    );
    assert_eq!(
        ColorMode::detect_from(env_of(&[
            ("TERM", "xterm-256color"),
            ("COLORTERM", "24bit")
        ])),
        ColorMode::TrueColor
    );
    assert_eq!(
        ColorMode::detect_from(env_of(&[("TERM", "xterm-256color")])),
        ColorMode::Ansi256,
        "unidentified terminal, no hint ⇒ pi's `256color` fallback"
    );
    assert_eq!(
        ColorMode::detect_from(env_of(&[("COLORTERM", "not-truecolor")])),
        ColorMode::Ansi256,
        "strict equality, not substring"
    );
}

/// Pi has no monochrome mode: `type ColorMode = "truecolor" | "256color"` (`theme.ts:167`). A dumb
/// or unset `TERM` still gets the full 256-colour UI. FAILS before the fix, which returned
/// `ColorMode::None` and stripped every role including the background tints.
#[test]
fn t3_dumb_or_missing_term_still_gets_colour() {
    assert_eq!(
        ColorMode::detect_from(env_of(&[("TERM", "dumb")])),
        ColorMode::Ansi256
    );
    assert_eq!(
        ColorMode::detect_from(env_of(&[("TERM", "")])),
        ColorMode::Ansi256
    );
    assert_eq!(ColorMode::detect_from(env_of(&[])), ColorMode::Ansi256);
    for env in [vec![("TERM", "dumb")], vec![("TERM", "")], vec![]] {
        assert_ne!(ColorMode::detect_from(env_of(&env)), ColorMode::None);
    }
}

/// MIRROR: `ColorMode::None` still exists and still strips colour when a caller asks for it
/// explicitly — the fix removes it from *detection*, not from the projection.
#[test]
fn t3_mirror_explicit_monochrome_projection_still_works() {
    assert_eq!(ColorMode::None.project(Color::Rgb(1, 2, 3)), Color::Reset);
    assert!(
        UiTheme::dark()
            .with_color_mode(ColorMode::None)
            .foreground
            .is_none()
    );
}

// ---------------------------------------------------------------------------------------------
// T5 / T6 — fenced-code highlighting.
// ---------------------------------------------------------------------------------------------

/// Every span with its **effective** style. ratatui resolves a cell as `line.style` patched by the
/// span's own style, and the two code paths under test use different levels: the whole-block
/// fallback is a `Line::styled(...)` (line level), while the per-scope highlight builds
/// `Line::from(spans)` (span level). Comparing only `span.style` would read the fallback as
/// uncoloured and quietly pass the very assertion T5 exists to make.
fn spans_of(lines: &[ratatui::text::Line<'static>]) -> Vec<(String, Style)> {
    lines
        .iter()
        .flat_map(|l| {
            l.spans
                .iter()
                .map(|s| (s.content.to_string(), l.style.patch(s.style)))
        })
        .collect()
}

fn style_of(lines: &[ratatui::text::Line<'static>], needle: &str) -> Style {
    spans_of(lines)
        .into_iter()
        .find(|(t, _)| t.trim() == needle)
        .unwrap_or_else(|| panic!("no span {needle:?} in {:?}", spans_of(lines)))
        .1
}

/// Pi pushes cli-highlight's output verbatim — `lines.push(`${indent}${hlLine}`)`,
/// `tui/src/components/markdown.ts:526` — and cli-highlight emits an escape only for the 24 classes
/// in `buildCliHighlightTheme` (`theme.ts:1119-1145`). A token it does not classify carries no
/// escape at all and sits at the terminal default.
///
/// FAILS before the fix: `markdown.rs` used `md_code_block_style()` as the per-span default, so
/// every unclassified run came out `mdCodeBlock` = `#b5bd68` green.
#[test]
fn t5_unclassified_code_tokens_carry_no_colour() {
    let t = UiTheme::dark();
    let lines = render_markdown("```rust\nlet total = compute(1);\n```", 80, &t);
    let green = Color::Rgb(0xb5, 0xbd, 0x68);

    // `total` is a bare identifier: syntect gives it no scope the table knows.
    let ident = style_of(&lines, "total");
    assert_eq!(
        ident.fg, None,
        "an unclassified token must inherit the terminal default"
    );
    assert_ne!(
        ident.fg,
        Some(green),
        "mdCodeBlock green is the WHOLE-BLOCK fallback, not this"
    );

    assert!(
        !spans_of(&lines)
            .iter()
            .any(|(txt, st)| st.fg == Some(green) && !txt.contains("```")),
        "no code body span may be mdCodeBlock green when the language highlighted"
    );
}

/// MIRROR: `mdCodeBlock` is still the whole-block fallback. Pi reaches it exactly twice — when the
/// language is absent/unsupported (`theme.ts:1275`) and when the highlighter throws (`:1284`) —
/// and then colours *every* line of the block.
#[test]
fn t5_mirror_unknown_language_still_falls_back_to_md_code_block() {
    let t = UiTheme::dark();
    let green = Color::Rgb(0xb5, 0xbd, 0x68);
    for src in [
        "```\nplain text here\n```",
        "```nosuchlang-zzz\nplain text here\n```",
    ] {
        let lines = render_markdown(src, 80, &t);
        let body = style_of(&lines, "plain text here");
        assert_eq!(
            body.fg,
            Some(green),
            "{src:?}: whole-block mdCodeBlock fallback"
        );
    }
    // And classified tokens in a known language keep their syntax roles.
    let rs = render_markdown("```rust\nlet total = compute(1);\n```", 80, &t);
    assert_eq!(
        style_of(&rs, "let").fg,
        Some(Color::Rgb(0x56, 0x9C, 0xD6)),
        "syntaxKeyword"
    );
    assert_eq!(
        style_of(&rs, "1").fg,
        Some(Color::Rgb(0xB5, 0xCE, 0xA8)),
        "syntaxNumber"
    );
}

/// Pi maps cli-highlight's `meta` class to `muted` — `meta: (s) => t.fg("muted", s)`,
/// `theme.ts:1128` — and highlight.js puts that one class on the entire Rust attribute / Python
/// decorator / C preprocessor line. `muted` is `gray` `#808080` (`dark.json:30`+`:11`).
///
/// FAILS before the fix: only `meta.attribute` was mapped (to `syntaxVariable`), so `#` and `]`
/// came out `syntaxPunctuation` and `derive` came out `#9CDCFE`, with the rest green.
#[test]
fn t6_annotation_and_preprocessor_lines_are_muted() {
    let t = UiTheme::dark();
    let muted = Color::Rgb(0x80, 0x80, 0x80);

    let rs = render_markdown("```rust\n#[derive(Debug)]\nstruct S;\n```", 80, &t);
    for tok in ["#", "[", "derive", "]"] {
        assert_eq!(
            style_of(&rs, tok).fg,
            Some(muted),
            "rust attribute token {tok:?}"
        );
    }

    let py = render_markdown("```python\n@decorator\ndef f():\n    pass\n```", 80, &t);
    for tok in ["@", "decorator"] {
        assert_eq!(
            style_of(&py, tok).fg,
            Some(muted),
            "python decorator token {tok:?}"
        );
    }

    let c = render_markdown("```c\n#include <stdio.h>\n```", 80, &t);
    assert_eq!(
        style_of(&c, "#include").fg,
        Some(muted),
        "c preprocessor directive"
    );
}

/// MIRROR: the `meta` container rule must NOT swallow ordinary code. syntect wraps plain statements
/// in `meta.function.*` / `meta.block.*` / `meta.group.*` / `meta.qualified-name.*`; a blanket
/// `meta` prefix would grey out most of a block, which is the opposite of pi's output.
#[test]
fn t6_mirror_structural_meta_scopes_do_not_grey_out_code() {
    let t = UiTheme::dark();
    let muted = Color::Rgb(0x80, 0x80, 0x80);
    let rs = render_markdown("```rust\nfn main() {\n    let s = \"hi\";\n}\n```", 80, &t);
    // `fn main() { … }` lives under `meta.function.rust` / `meta.block.rust` throughout.
    assert_eq!(
        style_of(&rs, "fn").fg,
        Some(Color::Rgb(0x56, 0x9C, 0xD6)),
        "storage → syntaxKeyword"
    );
    assert_eq!(
        style_of(&rs, "main").fg,
        Some(Color::Rgb(0xDC, 0xDC, 0xAA)),
        "syntaxFunction"
    );
    assert_ne!(style_of(&rs, "fn").fg, Some(muted));
    assert_ne!(style_of(&rs, "main").fg, Some(muted));
    let py = render_markdown("```python\nvalue = other\n```", 80, &t);
    // `other` sits under `meta.qualified-name.python` — unclassified upstream, so no colour.
    assert_ne!(
        style_of(&py, "other").fg,
        Some(muted),
        "a bare name is not a `meta` span in pi"
    );
}

// ---------------------------------------------------------------------------------------------
// T7 — the markdown link-URL suffix.
// ---------------------------------------------------------------------------------------------

/// `linkUrl` is `(text) => theme.fg("mdLinkUrl", text)` (`theme.ts:1256`), emitted at
/// `markdown.ts:705` with nothing wrapped around it. FAILS before the fix, which added
/// `Modifier::DIM`. `mdLinkUrl` is `dimGray` `#666666` (`dark.json:50`).
#[test]
fn t7_link_url_suffix_is_colour_only() {
    let s = UiTheme::dark().md_link_url_style();
    assert_eq!(
        s.fg,
        Some(Color::Rgb(0x66, 0x66, 0x66)),
        "mdLinkUrl = dimGray"
    );
    assert!(!s.add_modifier.contains(Modifier::DIM));
    assert_eq!(s.add_modifier, Modifier::empty());
}

/// MIRROR: the link *text* is genuinely underlined — `this.theme.link(this.theme.underline(
/// linkText))`, `markdown.ts:691` — so `md_link_style` must keep `UNDERLINED`.
#[test]
fn t7_mirror_link_text_keeps_its_underline() {
    let s = UiTheme::dark().md_link_style();
    assert!(s.add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(
        s.fg,
        Some(Color::Rgb(0x81, 0xa2, 0xbe)),
        "mdLink (dark.json:48)"
    );
}

// ---------------------------------------------------------------------------------------------
// T8 / T9 — accessors that ignored their own token.
// ---------------------------------------------------------------------------------------------

/// A theme whose message/tool tokens are all distinct from `text`, so an accessor that silently
/// reads `text` is caught. Values are arbitrary sentinels, not palette colours.
fn sentinel_theme() -> UiTheme {
    let mut t = UiTheme::dark();
    t.roles
        .insert("toolTitle".into(), Color::Rgb(0x11, 0x22, 0x33));
    t.roles
        .insert("userMessageText".into(), Color::Rgb(0x44, 0x55, 0x66));
    t.roles
        .insert("customMessageText".into(), Color::Rgb(0x77, 0x88, 0x99));
    t.roles
        .insert("customMessageLabel".into(), Color::Rgb(0xaa, 0xbb, 0xcc));
    t.roles
        .insert("borderMuted".into(), Color::Rgb(0xdd, 0xee, 0xff));
    t
}

/// `theme.fg("toolTitle", theme.bold("read"))` — `core/tools/read.ts:81`, and identically
/// `bash.ts:236`, `edit.ts:207`, `find.ts:80`, `grep.ts:84`, `ls.ts:60`, `write.ts:146`,
/// `components/tool-execution.ts:136,366`. FAILS before the fix, which read `self.foreground`.
#[test]
fn t8_tool_title_reads_the_tool_title_token() {
    let t = sentinel_theme();
    let s = t.tool_title_style();
    assert_eq!(
        s.fg,
        Some(Color::Rgb(0x11, 0x22, 0x33)),
        "toolTitle, not text"
    );
    assert_ne!(
        s.fg, t.foreground,
        "the `text` role must not be what is drawn"
    );
    assert!(
        s.add_modifier.contains(Modifier::BOLD),
        "theme.bold() wraps the title upstream"
    );
}

/// MIRROR: both built-ins alias `"toolTitle": "text"` (`dark.json:45`, `light.json:44`), so the
/// rendered result there is unchanged by the fix.
#[test]
fn t8_mirror_builtin_tool_title_still_equals_text() {
    for t in [UiTheme::dark(), UiTheme::light()] {
        assert_eq!(t.tool_title_style().fg, t.foreground);
    }
}

/// `userMessageText` — `{ color: (content) => theme.fg("userMessageText", content) }`,
/// `components/user-message.ts:48`. `customMessageText` — `custom-message.ts:109`.
/// `customMessageLabel` — `custom-message.ts:92` (and the same `\x1b[1m[…]\x1b[22m` bold form in
/// `skill-invocation-message.ts:38`, `branch-summary-message.ts:35`,
/// `compaction-summary-message.ts:36`). `borderMuted` — `getEditorTheme()`, `theme.ts:1303`.
/// All four were parsed into `UiTheme.roles` and never read.
#[test]
fn t9_message_and_border_tokens_are_actually_read() {
    let t = sentinel_theme();

    assert_eq!(
        t.user_message_bg_style().fg,
        Some(Color::Rgb(0x44, 0x55, 0x66)),
        "userMessageText"
    );
    assert!(
        t.user_message_bg_style().bg.is_some(),
        "userMessageBg still applied"
    );

    assert_eq!(
        t.custom_message_bg_style().fg,
        Some(Color::Rgb(0x77, 0x88, 0x99)),
        "customMessageText — this used to be dim_style()"
    );
    assert_ne!(t.custom_message_bg_style().fg, t.dim_style().fg);

    let label = t.custom_message_label_style();
    assert_eq!(
        label.fg,
        Some(Color::Rgb(0xaa, 0xbb, 0xcc)),
        "customMessageLabel"
    );
    assert!(
        label.add_modifier.contains(Modifier::BOLD),
        "\\x1b[1m…\\x1b[22m is SGR bold"
    );
    assert_ne!(label.fg, t.accent, "the label is not the accent role");

    assert_eq!(
        t.border_muted_style().fg,
        Some(Color::Rgb(0xdd, 0xee, 0xff)),
        "borderMuted"
    );
    assert_ne!(
        t.border_muted_style().fg,
        t.border,
        "borderMuted is not `border`"
    );
}

/// The built-in `customMessageLabel` is purple, distinctly not the teal accent cyrup used to draw
/// (`dark.json:41` `#9575cd`, `light.json:40` `#7e57c2`).
#[test]
fn t9_builtin_custom_message_label_is_purple_not_accent() {
    assert_eq!(
        UiTheme::dark().custom_message_label_style().fg,
        Some(Color::Rgb(0x95, 0x75, 0xcd))
    );
    assert_eq!(
        UiTheme::light().custom_message_label_style().fg,
        Some(Color::Rgb(0x7e, 0x57, 0xc2))
    );
    assert_ne!(
        UiTheme::dark().custom_message_label_style().fg,
        UiTheme::dark().accent
    );
}

/// MIRROR: a theme that omits the new tokens still renders — the accessors fall back rather than
/// dropping colour (Pi's own `Theme` throws on an unknown token; cyrup's no-panic policy forbids
/// that, so the fallback chain is the cyrup-side mechanism difference).
#[test]
fn t9_mirror_missing_tokens_fall_back_instead_of_panicking() {
    let mut t = UiTheme::dark();
    for k in [
        "userMessageText",
        "customMessageText",
        "customMessageLabel",
        "borderMuted",
    ] {
        t.roles.remove(k);
    }
    assert_eq!(
        t.user_message_bg_style().fg,
        t.foreground,
        "falls back to `text`"
    );
    assert_eq!(t.custom_message_bg_style().fg, t.dim_style().fg);
    assert!(t.custom_message_label_style().fg.is_some());
    assert_eq!(
        t.border_muted_style().fg,
        t.border,
        "falls back to `border`"
    );
}

/// `getThinkingBorderColor`'s `default:` arm is `thinkingOff` (`theme.ts:437-438`), not the
/// `border` role. Found while wiring `borderMuted`; same class of defect as T8.
#[test]
fn thinking_border_unknown_level_falls_back_to_thinking_off() {
    let t = UiTheme::dark();
    assert_eq!(
        t.thinking_border_style("ultra"),
        t.thinking_border_style("off")
    );
    assert_eq!(
        t.thinking_border_style("ultra").fg,
        Some(Color::Rgb(0x50, 0x50, 0x50))
    );
    assert_ne!(t.thinking_border_style("ultra"), t.border_style());
}

// ---------------------------------------------------------------------------------------------
// T9 at the render sites — the accessors above must actually reach the screen.
// ---------------------------------------------------------------------------------------------

/// The `[skill]` / `[custom]` / `[branch]` / `[compaction]` bracket is `customMessageLabel`, not
/// `accent`. All four upstream components build it the same way —
/// `theme.fg("customMessageLabel", "\x1b[1m[<name>]\x1b[22m")` — at
/// `skill-invocation-message.ts:38`, `custom-message.ts:92`, `branch-summary-message.ts:35`,
/// `compaction-summary-message.ts:36`. FAILS before the fix, which drew bold `accent`.
#[test]
fn t9_render_labeled_block_bracket_uses_custom_message_label() {
    use crate::App;
    use ratatui::backend::TestBackend;

    let theme = UiTheme::dark();
    let purple = Color::Rgb(0x95, 0x75, 0xcd);
    let mut app = App::new(TestBackend::new(80, 24), theme.clone()).unwrap();
    app.transcript_mut()
        .push_skill_invocation("commit-helper", "body");
    app.transcript_mut().push_branch_summary("body");
    app.transcript_mut().push_compaction_summary(1_000, "body");
    app.draw().unwrap();

    let mut seen = 0usize;
    for line in app.scrollback_lines() {
        for span in &line.spans {
            let txt = span.content.as_ref();
            if matches!(txt, "[skill]" | "[branch]" | "[compaction]") {
                let st = line.style.patch(span.style);
                assert_eq!(st.fg, Some(purple), "{txt} bracket is customMessageLabel");
                assert_ne!(
                    st.fg, theme.accent,
                    "{txt} bracket must not be the accent role"
                );
                assert!(
                    st.add_modifier.contains(Modifier::BOLD),
                    "{txt} keeps \\x1b[1m"
                );
                seen += 1;
            }
        }
    }
    assert_eq!(seen, 3, "all three labeled blocks reached scrollback");
}

/// Pi's `ExtensionEditorComponent` is `new Editor(tui, getEditorTheme(), options)`
/// (`components/extension-editor.ts:70`) and nothing reassigns its `borderColor`, so its rule stays
/// `getEditorTheme().borderColor` = `theme.fg("borderMuted", …)` (`theme.ts:1301-1304`). Only the
/// chat editor is repainted per reasoning level (`interactive-mode.ts:3990-3993`).
///
/// FAILS before the fix: the dialog inherited `InputEditor`'s `"medium"` thinking colour
/// (`thinkingMedium` = `#81a2be`) instead of `borderMuted` (`darkGray` = `#505050`).
#[test]
fn t9_render_extension_editor_rule_is_border_muted() {
    use crate::InputEditor;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    let theme = UiTheme::dark();
    let border_muted = Color::Rgb(0x50, 0x50, 0x50);
    let thinking_medium = Color::Rgb(0x81, 0xa2, 0xbe);

    let render = |muted: bool| -> Color {
        let mut editor = InputEditor::new();
        editor.set_text("hello");
        if muted {
            editor.use_muted_border();
        }
        let mut term = Terminal::new(TestBackend::new(40, 6)).unwrap();
        term.draw(|f| {
            use crate::Component;
            editor.render(f, Rect::new(0, 0, 40, 4), &theme);
        })
        .unwrap();
        // The `Block`'s TOP border row: the rule glyph carries the border style.
        term.backend().buffer()[(0, 0)].fg
    };

    assert_eq!(
        render(true),
        border_muted,
        "extension editor keeps getEditorTheme().borderColor"
    );
    assert_eq!(
        render(false),
        thinking_medium,
        "MIRROR: the chat editor still signals the reasoning level on its rule"
    );
}

/// The `role_style` hex fallbacks (used only by the synthetic no-resource theme) must agree with the
/// real palette, so a degraded theme is a dimmer version of the same design rather than a different
/// one. Five had drifted: `mdCodeBlockBorder`/`mdQuote`/`mdQuoteBorder`/`mdHr` are `gray` `#808080`
/// (`dark.json:53,54,55,56` — `:57` is `mdListBullet`) and `mdLinkUrl` is `dimGray` `#666666`
/// (`dark.json:50`). Found while fixing T7.
///
/// Asserted for **both** built-ins. `builtin_or_static` synthesizes `UiTheme::dark()` and
/// `UiTheme::light()` with an empty `roles` map, so both fall through to the same accessor;
/// aligning the hexes to `dark.json` alone made a resource-less light theme draw dark-theme greys
/// (`light.json:52-55` is `mediumGray` `#6c6c6c`, `:49` is `dimGray` `#767676`). The light half
/// FAILS before the theme-aware fallback.
#[test]
fn role_style_hex_fallbacks_match_the_real_palette() {
    for real in [UiTheme::dark(), UiTheme::light()] {
        let which = real.name.clone();
        let mut bare = real.clone();
        bare.roles.clear();
        bare.muted = None;
        for (name, got, want) in [
            (
                "mdCodeBlockBorder",
                bare.md_code_block_border_style(),
                real.md_code_block_border_style(),
            ),
            ("mdQuote", bare.md_quote_style(), real.md_quote_style()),
            (
                "mdQuoteBorder",
                bare.md_quote_border_style(),
                real.md_quote_border_style(),
            ),
            ("mdHr", bare.md_hr_style(), real.md_hr_style()),
            (
                "mdLinkUrl",
                bare.md_link_url_style(),
                real.md_link_url_style(),
            ),
            (
                "mdHeading",
                bare.md_heading_style(),
                real.md_heading_style(),
            ),
            (
                "mdCodeBlock",
                bare.md_code_block_style(),
                real.md_code_block_style(),
            ),
            ("mdCode", bare.md_code_style(), real.md_code_style()),
            ("mdLink", bare.md_link_style(), real.md_link_style()),
            (
                "mdListBullet",
                bare.md_list_bullet_style(),
                real.md_list_bullet_style(),
            ),
            ("dim", bare.dim_style(), real.dim_style()),
            ("muted", bare.muted_style(), real.muted_style()),
            (
                "thinkingText",
                bare.thinking_text_style(),
                real.thinking_text_style(),
            ),
            (
                "toolDiffAdded",
                bare.tool_diff_added_style(),
                real.tool_diff_added_style(),
            ),
            (
                "toolDiffRemoved",
                bare.tool_diff_removed_style(),
                real.tool_diff_removed_style(),
            ),
            (
                "toolDiffContext",
                bare.tool_diff_context_style(),
                real.tool_diff_context_style(),
            ),
        ] {
            assert_eq!(
                got.fg, want.fg,
                "{which}/{name}: fallback hex must equal the palette value"
            );
        }
        // The syntax table's hexes are the other half of the same accessor (`dark.json:63-71` /
        // `light.json:62-70`); syntect scope -> Pi class per `buildCliHighlightTheme`.
        for (scope, name) in [
            ("comment.line.rust", "syntaxComment"),
            ("string.quoted.double.rust", "syntaxString"),
            ("constant.numeric.integer.rust", "syntaxNumber"),
            ("entity.name.function.rust", "syntaxFunction"),
            ("entity.name.type.rust", "syntaxType"),
            ("keyword.operator.rust", "syntaxOperator"),
            ("keyword.control.rust", "syntaxKeyword"),
            ("variable.other.rust", "syntaxVariable"),
            ("punctuation.terminator.rust", "syntaxPunctuation"),
        ] {
            assert_eq!(
                bare.syntax_style_for_scope(scope).and_then(|s| s.fg),
                real.syntax_style_for_scope(scope).and_then(|s| s.fg),
                "{which}/{name}: syntax fallback hex must equal the palette value"
            );
        }
        // The thinking-border ladder, `dark.json:73-78` / `light.json:72-77`. `thinkingOff` used to
        // default to `#666666` where the token is `darkGray` `#505050` / `lightGray` `#b0b0b0`.
        for level in ["off", "minimal", "low", "medium", "high", "xhigh"] {
            assert_eq!(
                bare.thinking_border_style(level).fg,
                real.thinking_border_style(level).fg,
                "{which}/thinking{level}: fallback hex must equal the palette value"
            );
        }
        // `max` is deliberately NOT in that list. Pi makes `thinkingMax` the one OPTIONAL colour
        // token and resolves it as `colors.thinkingMax ?? colors.thinkingXhigh`
        // (`theme.ts:93,326-329,358`), so a palette that omits it reuses its OWN `xhigh` rather
        // than any hardcoded default. `dark.json:79` and `light.json:78` do define it, which is
        // why the bare and real values differ here — that difference is the ported `??`, not drift.
        assert_eq!(
            bare.thinking_border_style("max").fg,
            bare.thinking_border_style("xhigh").fg,
            "{which}/thinkingMax: `colors.thinkingMax ?? colors.thinkingXhigh` (theme.ts:329)"
        );
        assert_ne!(
            real.thinking_border_style("max").fg,
            real.thinking_border_style("xhigh").fg,
            "{which}: the built-in DOES define thinkingMax"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Attribute-only syntax classes — `emphasis` / `strong` / `link`.
// ---------------------------------------------------------------------------------------------

/// `buildCliHighlightTheme` builds three of its 24 classes from the bare `chalk` combinators, with
/// **no** `fg()` call at all: `emphasis: (s) => t.italic(s)` (`theme.ts:1140`),
/// `strong: (s) => t.bold(s)` (`:1141`), `link: (s) => t.underline(s)` (`:1142`) — and
/// `Theme.italic`/`bold`/`underline` are `chalk.italic`/`chalk.bold`/`chalk.underline`
/// (`theme.ts:384-394`).
///
/// FAILS before the fix: `markup.italic` and `markup.bold` returned an explicit `text` foreground
/// (`#d4d4d4`) alongside the attribute, which overrides whatever colour the surrounding run had.
/// `markup.underline` returned `None` (Pi's `link` class was unmapped).
#[test]
fn attribute_only_syntax_classes_emit_no_foreground() {
    for t in [UiTheme::dark(), UiTheme::light()] {
        for (scope, want, class) in [
            ("markup.italic.markdown", Modifier::ITALIC, "emphasis"),
            ("markup.bold.markdown", Modifier::BOLD, "strong"),
            (
                "markup.underline.link.markdown",
                Modifier::UNDERLINED,
                "link",
            ),
        ] {
            let s = t
                .syntax_style_for_scope(scope)
                .unwrap_or_else(|| panic!("{scope} unmapped"));
            assert_eq!(s.fg, None, "{}/{class}: Pi never calls fg() here", t.name);
            assert_eq!(s.bg, None, "{}/{class}: no background either", t.name);
            assert_eq!(
                s.add_modifier, want,
                "{}/{class}: the SGR attribute alone",
                t.name
            );
        }
        assert_ne!(
            t.syntax_style_for_scope("markup.italic.markdown")
                .and_then(|s| s.fg),
            t.foreground,
            "{}: the `text` role must not be painted",
            t.name
        );
    }
}

/// MIRROR: the two `markup.*` classes Pi DOES colour keep their foreground —
/// `addition: (s) => t.fg("toolDiffAdded", s)` / `deletion: (s) => t.fg("toolDiffRemoved", s)`
/// (`theme.ts:1143-1144`).
#[test]
fn mirror_markup_diff_classes_keep_their_colour() {
    let t = UiTheme::dark();
    let add = t.syntax_style_for_scope("markup.inserted.diff").unwrap();
    let del = t.syntax_style_for_scope("markup.deleted.diff").unwrap();
    assert_eq!(
        add.fg,
        Some(Color::Rgb(0xb5, 0xbd, 0x68)),
        "toolDiffAdded (dark.json:59)"
    );
    assert_eq!(
        del.fg,
        Some(Color::Rgb(0xcc, 0x66, 0x66)),
        "toolDiffRemoved (dark.json:60)"
    );
    assert_eq!(
        add.add_modifier,
        Modifier::empty(),
        "fg() emits no SGR attribute"
    );
    assert_eq!(del.add_modifier, Modifier::empty());
}

/// The attribute-only classes at the RENDER site: a fenced ```md block's `*em*` / `**strong**`
/// runs must reach scrollback carrying the attribute and no foreground of their own.
#[test]
fn attribute_only_syntax_classes_reach_the_code_block_unpainted() {
    let t = UiTheme::dark();
    let lines = render_markdown("```md\n*em* and **strong**\n```", 60, &t);
    let mut em = 0usize;
    let mut strong = 0usize;
    for line in &lines {
        for span in &line.spans {
            match span.content.as_ref() {
                "em" => {
                    assert!(
                        span.style.add_modifier.contains(Modifier::ITALIC),
                        "emphasis italic"
                    );
                    assert_eq!(span.style.fg, None, "emphasis carries no colour");
                    em += 1;
                }
                "strong" => {
                    assert!(
                        span.style.add_modifier.contains(Modifier::BOLD),
                        "strong bold"
                    );
                    assert_eq!(span.style.fg, None, "strong carries no colour");
                    strong += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!((em, strong), (1, 1), "both runs rendered:\n{lines:?}");
}

// ---------------------------------------------------------------------------------------------
// T6 continued — the `meta` container must not swallow a nested literal.
// ---------------------------------------------------------------------------------------------

/// Find the style of the span whose text is exactly `needle`.
fn span_style(lines: &[ratatui::text::Line<'static>], needle: &str) -> Option<Style> {
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == needle)
        .map(|s| s.style)
}

/// Pi maps highlight.js's `meta` CLASS to `muted` (`theme.ts:1128`) — but highlight.js does not emit
/// one flat `meta` span: its `meta` modes declare sub-modes and cli-highlight wraps each in its own
/// class, so a string inside a Rust attribute or a C `#include` arrives as `string` and is painted
/// `syntaxString` (`theme.ts:1125`) while the construct around it stays grey.
///
/// syntect nests the literal *inside* `meta.annotation.rust` / `meta.preprocessor.include.c`, and
/// the container pre-pass returned on the FIRST such scope on the stack — so every token of the
/// construct, literals included, came back `muted`. FAILS before the narrowing.
#[test]
fn meta_container_does_not_swallow_a_nested_literal() {
    let t = UiTheme::dark();
    let muted = t.muted_style().fg;
    let string = t
        .syntax_style_for_scope("string.quoted.double.rust")
        .and_then(|s| s.fg);
    assert_ne!(muted, string, "the two roles must be distinguishable");

    // Rust: `#[cfg(feature = "wasm-host")]` — syntect scopes the literal
    // `meta.annotation.rust > meta.annotation.parameters.rust > meta.group.rust >
    //  string.quoted.double.rust`.
    let rs = render_markdown("```rust\n#[cfg(feature = \"wasm-host\")]\n```", 60, &t);
    assert_eq!(
        span_style(&rs, "wasm-host").and_then(|s| s.fg),
        string,
        "literal keeps syntaxString"
    );
    assert_eq!(
        span_style(&rs, "cfg").and_then(|s| s.fg),
        muted,
        "annotation identifier is muted"
    );
    assert_eq!(
        span_style(&rs, "#").and_then(|s| s.fg),
        muted,
        "annotation punctuation is muted"
    );
    assert_eq!(
        span_style(&rs, "]").and_then(|s| s.fg),
        muted,
        "closing bracket is muted"
    );

    // C: `#include <stdio.h>` — the bracketed header is
    // `meta.preprocessor.include.c > string.quoted.other.lt-gt.include.c`.
    let c = render_markdown("```c\n#include <stdio.h>\n```", 60, &t);
    assert_eq!(
        span_style(&c, "stdio.h").and_then(|s| s.fg),
        string,
        "include header is a string"
    );
    assert_eq!(
        span_style(&c, "<").and_then(|s| s.fg),
        string,
        "its delimiters too"
    );
    assert_eq!(
        span_style(&c, "#include").and_then(|s| s.fg),
        muted,
        "the directive is muted"
    );
}

/// MIRROR: everything the container is *for* still greys. A Rust attribute with no literal in it,
/// and the C `#define N 42` whose `42` highlight.js leaves inside the `meta` mode (its C
/// preprocessor mode contains string and comment sub-modes, not a number one).
#[test]
fn mirror_meta_container_still_greys_the_whole_annotation() {
    let t = UiTheme::dark();
    let muted = t.muted_style().fg;
    let rs = render_markdown("```rust\n#[derive(Debug)]\n```", 60, &t);
    for needle in ["#", "[", "derive", "(", "Debug", ")", "]"] {
        assert_eq!(
            span_style(&rs, needle).and_then(|s| s.fg),
            muted,
            "`{needle}` of #[derive(Debug)] is muted:\n{rs:?}"
        );
    }
    let c = render_markdown("```c\n#define N 42\n```", 60, &t);
    for needle in ["#define", "N", "42"] {
        assert_eq!(
            span_style(&c, needle).and_then(|s| s.fg),
            muted,
            "`{needle}` is muted:\n{c:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// T9 continued — `customMessageBg` / `customMessageText` must reach the screen.
// ---------------------------------------------------------------------------------------------

/// The user action: an extension posts a custom message during a turn (a review/notice block). It
/// arrives as `AgentSessionEvent::MessageEnd { message: AgentMessage::Custom { .. } }`, which
/// `App::ingest_event` folds into a labeled transcript block (`app.rs` `displayable_custom_message_from_event`
/// → `push_custom_message_rendered`) — the same path a replayed session takes via
/// `AgentMessage::Custom` when `display` is set. Pi's component is
/// `new Box(1, 1, (t) => theme.bg("customMessageBg", t))` holding the `[label]` `Text` and
/// `new Markdown(text, 0, 0, this.markdownTheme, { color: (text) => theme.fg("customMessageText",
/// text) })` — `components/custom-message.ts:36`, `:92`, `:107-111`.
///
/// FAILS before the fix: `UiTheme::custom_message_bg_style()` had zero callers in
/// `crates/cyrup-tui/src/`, so the block drew no fill and its body took the plain `text` role.
#[test]
fn t9_render_custom_message_paints_bg_and_text_tokens() {
    use crate::App;
    use cyrup_agent::AgentMessage;
    use cyrup_session_svc::AgentSessionEvent;
    use ratatui::backend::TestBackend;

    let fill = Color::Rgb(0x12, 0x34, 0x56);
    let body_fg = Color::Rgb(0x77, 0x88, 0x99);
    let mut theme = UiTheme::dark();
    theme.roles.insert("customMessageBg".into(), fill);
    theme.roles.insert("customMessageText".into(), body_fg);

    let mut app = App::new(TestBackend::new(80, 24), theme.clone()).unwrap();
    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: AgentMessage::Custom {
            kind: "review".to_string(),
            payload: serde_json::json!("Looks good to ship."),
            details: None,
            display: true,
            timestamp: None,
        },
    });
    app.draw().unwrap();

    let mut label_bg = None;
    let mut body = None;
    for line in app.scrollback_lines() {
        for span in &line.spans {
            let st = line.style.patch(span.style);
            if span.content.as_ref() == "[review]" {
                label_bg = Some(st.bg);
            }
            if span.content.as_ref().contains("Looks good to ship") {
                body = Some(st);
            }
        }
    }
    let body = body.expect("custom body reached scrollback");
    assert_eq!(
        label_bg,
        Some(Some(fill)),
        "customMessageBg fills the label row (Box, :36)"
    );
    assert_eq!(
        body.bg,
        Some(fill),
        "customMessageBg fills the body rows too"
    );
    assert_eq!(
        body.fg,
        Some(body_fg),
        "customMessageText colours the Markdown body (:109)"
    );
    assert_ne!(
        body.fg, theme.foreground,
        "the plain `text` role must not be what is drawn"
    );
}

/// MIRROR: a theme that omits `customMessageBg` leaves the block on the terminal default ground
/// rather than inventing a fill, and the three sibling components Pi backgrounds with the SAME
/// token (`skill-invocation-message.ts:17`, `branch-summary-message.ts:16`,
/// `compaction-summary-message.ts:16`) get it too.
#[test]
fn t9_mirror_custom_message_bg_covers_the_sibling_blocks_and_is_optional() {
    use crate::App;
    use ratatui::backend::TestBackend;

    let fill = Color::Rgb(0x12, 0x34, 0x56);
    let mut theme = UiTheme::dark();
    theme.roles.insert("customMessageBg".into(), fill);
    let mut app = App::new(TestBackend::new(80, 24), theme).unwrap();
    app.transcript_mut()
        .push_skill_invocation("commit-helper", "body");
    app.transcript_mut().push_branch_summary("body");
    app.transcript_mut().push_compaction_summary(1_000, "body");
    app.draw().unwrap();
    let mut seen = 0usize;
    for line in app.scrollback_lines() {
        for span in &line.spans {
            if matches!(
                span.content.as_ref(),
                "[skill]" | "[branch]" | "[compaction]"
            ) {
                assert_eq!(line.style.patch(span.style).bg, Some(fill));
                seen += 1;
            }
        }
    }
    assert_eq!(seen, 3, "all three sibling blocks carry the fill");

    let mut bare = UiTheme::dark();
    bare.roles.remove("customMessageBg");
    let mut app = App::new(TestBackend::new(80, 24), bare).unwrap();
    app.transcript_mut().push_branch_summary("body");
    app.draw().unwrap();
    for line in app.scrollback_lines() {
        for span in &line.spans {
            if span.content.as_ref() == "[branch]" {
                assert_eq!(line.style.patch(span.style).bg, None, "no fill invented");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// T9 — `scrollbarThumb`, Pi's seventh (and OPTIONAL) background token.
//
// This is a direct mirror of `pi/packages/coding-agent/test/scrollbar-theme.test.ts`, which was RUN
// against v0.84.1's source to read its expectations. That test constructs no `ScrollView`, enters no
// alt-screen renderer and reads no `fullscreenScrollbar` setting: it loads a theme JSON and asserts
// how the token RESOLVES. Resolution is the part cyrup can port today, and it is the part a user
// notices first — a theme that omits the token must resolve to its own `selectedBg`, not to "no
// colour", and a theme that sets it must win.
//
// ```ts
// // :31-38
// delete themeJson.colors.scrollbarThumb;
// const loadedTheme = loadThemeFromPath(writeTheme(themeJson), "truecolor");
// expect(loadedTheme.getBgAnsi("scrollbarThumb")).toBe(loadedTheme.getBgAnsi("selectedBg"));
// // :40-47
// themeJson.colors.scrollbarThumb = "#123456";
// expect(loadedTheme.getBgAnsi("scrollbarThumb")).toBe("\x1b[48;2;18;52;86m");  // rgb(18,52,86)
// ```
// ---------------------------------------------------------------------------------------------

/// Load the built-in `dark` theme, optionally overriding `colors.scrollbarThumb`, and project it the
/// way boot does. `None` = the token is absent, which is upstream's "legacy theme" case.
fn dark_with_scrollbar_thumb(value: Option<&str>) -> UiTheme {
    let mut theme = cyrup_resources::theme::builtin_themes()
        .into_iter()
        .find(|t| t.data.name == "dark")
        .expect("the built-in `dark` theme");
    match value {
        Some(v) => {
            theme
                .data
                .colors
                .insert("scrollbarThumb".to_string(), v.to_string());
        }
        None => {
            theme.data.colors.remove("scrollbarThumb");
        }
    }
    UiTheme::from_resolved(theme.data.name.clone(), &theme.resolve(), 0)
}

/// `scrollbarThumb ?? selectedBg` — applied by `withThemeColorFallbacks` (`theme.ts:330`) and again
/// by the `Theme` constructor (`theme.ts:365`). A theme that omits the token resolves to its OWN
/// `selectedBg`, never to `None`.
///
/// FAILS before the fix: `BackgroundTheme` had no `scrollbar_thumb` field at all, and the raw role
/// lookup for a token the theme does not define answers `None` — i.e. "terminal default", which is
/// a different colour from `selectedBg` on every theme.
#[test]
fn t9_an_omitted_scrollbar_thumb_falls_back_to_selected_bg() {
    let theme = dark_with_scrollbar_thumb(None);
    let bg = theme.backgrounds();

    assert_eq!(
        bg.scrollbar_thumb, bg.selected,
        "`scrollbarThumb: bgColors.scrollbarThumb ?? bgColors.selectedBg` (theme.ts:365)"
    );
    // …and pin the concrete value, so the assertion cannot be satisfied by both being `None`.
    assert_eq!(
        bg.scrollbar_thumb,
        Some(Color::Rgb(0x3a, 0x3a, 0x4a)),
        "dark `selectedBg` is `#3a3a4a` (dark.json vars `selectedBg`)"
    );
}

/// The other half: an explicitly configured `scrollbarThumb` WINS over the fallback
/// (`scrollbar-theme.test.ts:40-47`, whose `#123456` is `rgb(18,52,86)`).
#[test]
fn t9_an_explicit_scrollbar_thumb_overrides_the_fallback() {
    let theme = dark_with_scrollbar_thumb(Some("#123456"));
    let bg = theme.backgrounds();

    assert_eq!(
        bg.scrollbar_thumb,
        Some(Color::Rgb(18, 52, 86)),
        "the theme's own value, not `selectedBg`"
    );
    assert_ne!(
        bg.scrollbar_thumb, bg.selected,
        "the fallback must not shadow an explicit token"
    );
}

/// The token is OPTIONAL upstream (`Type.Optional(ColorValueSchema)`, `theme.ts:50`), so a theme
/// that omits it must still LOAD — it is not one of the 51 required tokens. Guards against
/// "porting" the token by adding it to the required set, which would reject every pre-existing user
/// theme, the exact regression upstream's optionality exists to prevent.
#[test]
fn t9_scrollbar_thumb_is_optional_and_never_required() {
    assert!(
        !cyrup_resources::theme::REQUIRED_COLOR_TOKENS.contains(&"scrollbarThumb"),
        "`theme.ts:50` declares it Type.Optional; docs/themes.md:144 lists it with `thinkingMax` \
         as the two optional tokens among 51 required ones"
    );
    // A theme with no `scrollbarThumb` still resolves every other background.
    let bg = dark_with_scrollbar_thumb(None).backgrounds();
    assert!(bg.selected.is_some() && bg.user_message.is_some() && bg.tool_error.is_some());
}
