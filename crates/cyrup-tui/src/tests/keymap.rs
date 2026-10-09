//! Key parsing + keymap resolution (R-10-018 / R-10-023 / R-10-024).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{Action, Key, Keymap};

#[test]
fn parse_string_key_specs() {
    assert_eq!(Key::parse("ctrl+c").unwrap(), Key::ctrl('c'));
    let shift_tab = Key::parse("shift+tab").unwrap();
    assert_eq!(shift_tab.code, KeyCode::Tab);
    assert_eq!(shift_tab.mods, KeyModifiers::SHIFT);
    assert_eq!(Key::parse("esc").unwrap(), Key::plain(KeyCode::Esc));
    let alt_enter = Key::parse("alt+enter").unwrap();
    assert_eq!(alt_enter.code, KeyCode::Enter);
    assert_eq!(alt_enter.mods, KeyModifiers::ALT);
}

#[test]
fn parse_rejects_empty_and_modifier_only() {
    assert!(Key::parse("").is_err());
    assert!(Key::parse("ctrl+").is_err());
}

#[test]
fn key_matches_event() {
    let ev = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(Key::ctrl('c').matches(&ev));
    assert!(!Key::plain(KeyCode::Char('c')).matches(&ev));
}

#[test]
fn matches_ignores_lock_and_unsupported_modifier_masks() {
    // Pi strips the Caps/Num lock mask (and any unsupported modifier bit) before comparing
    // (keys.ts:361,656,779). crossterm surfaces `HYPER`/`META` as the closest analogues to the JS
    // `LOCK_MASK`; a Ctrl+D chord with a stray lock/hyper bit still resolves to the exit binding.
    let d_with_hyper = KeyEvent::new(
        KeyCode::Char('d'),
        KeyModifiers::CONTROL | KeyModifiers::HYPER,
    );
    assert_eq!(
        Keymap::default().action_for(&d_with_hyper),
        Some(Action::Quit)
    );
    // A bare key carrying only a lock/hyper bit still matches a no-modifier binding.
    let a_with_meta = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::META);
    assert!(Key::plain(KeyCode::Char('a')).matches(&a_with_meta));
}

#[test]
fn matches_normalizes_shifted_letters() {
    // Pi normalizes a shifted ASCII letter to its lowercase codepoint (keys.ts:360-366): a `shift+a`
    // binding matches a terminal reporting `Char('A')` + SHIFT (the disambiguate/Kitty path).
    let shift_a_upper = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
    let binding = Key {
        code: KeyCode::Char('a'),
        mods: KeyModifiers::SHIFT,
    };
    assert!(binding.matches(&shift_a_upper));
    // Symmetric: a spec written as `shift+A` matches a `Char('a')` + SHIFT event too.
    let shift_a_lower = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SHIFT);
    let binding_upper = Key {
        code: KeyCode::Char('A'),
        mods: KeyModifiers::SHIFT,
    };
    assert!(binding_upper.matches(&shift_a_lower));
    // Without shift, an uppercase letter is a distinct key (no spurious collapse).
    let plain_upper = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE);
    assert!(!Key::plain(KeyCode::Char('a')).matches(&plain_upper));
}

#[test]
fn default_keymap_binds_pi_app_actions() {
    // Pi defaults (core/keybindings.ts:63-202): Ctrl+D exit, Ctrl+C clear, Esc interrupt.
    let km = Keymap::default();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let plain_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(km.action_for(&ctrl_d), Some(Action::Quit));
    assert_eq!(km.action_for(&ctrl_c), Some(Action::Clear));
    assert_eq!(km.action_for(&esc), Some(Action::Interrupt));
    assert_eq!(km.action_for(&plain_a), None);
}

#[test]
fn keymap_rebind_overrides() {
    let mut km = Keymap::empty();
    km.bind(Key::ctrl('q'), Action::Quit);
    let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(km.action_for(&ctrl_q), Some(Action::Quit));
    // Rebinding the same key replaces, not duplicates.
    km.bind(Key::ctrl('q'), Action::Interrupt);
    assert_eq!(km.action_for(&ctrl_q), Some(Action::Interrupt));
}

// ======================================================= TUI-008 — the seven unbound ids ====

/// **TUI-008.** `interactive-mode.ts:2608-2618` @v0.83.0 registers seven app ids cyrup's
/// `Action::from_id` did not recognize, so a `keybindings.json` naming any of them was silently
/// dropped on the floor by `merge_json` and the documented default chords were dead keys.
///
/// FAILS before the fix: every `from_id` below returns `None`.
#[test]
fn tui008_the_seven_upstream_app_ids_resolve() {
    // `core/keybindings.ts:85, :87-90, :99-102, :115-118` @v0.83.0.
    assert_eq!(
        Action::from_id("app.model.select"),
        Some(Action::ModelSelect)
    );
    assert_eq!(
        Action::from_id("app.thinking.toggle"),
        Some(Action::ThinkingToggle)
    );
    assert_eq!(
        Action::from_id("app.message.copy"),
        Some(Action::MessageCopy)
    );
    assert_eq!(Action::from_id("app.session.new"), Some(Action::SessionNew));
    assert_eq!(
        Action::from_id("app.session.tree"),
        Some(Action::SessionTree)
    );
    assert_eq!(
        Action::from_id("app.session.fork"),
        Some(Action::SessionFork)
    );
    assert_eq!(
        Action::from_id("app.session.resume"),
        Some(Action::SessionResume)
    );
    // MIRROR — an id that genuinely is not a global app binding must still be `None`, so the arm
    // above is not a catch-all that would make every id "work".
    assert_eq!(Action::from_id("app.thinking.togglee"), None);
    assert_eq!(Action::from_id("tui.editor.undo"), None);
}

/// **TUI-008, the defaults half.** Three of the seven carry default chords upstream; four are
/// declared `defaultKeys: []` and MUST stay unbound — inventing a default would be a divergence,
/// and `keys_label` returning `None` is upstream's `keys.length === 0 → ""`
/// (`keybinding-hints.ts:30`).
#[test]
fn tui008_default_chords_match_upstream_including_the_deliberately_unbound_four() {
    let km = Keymap::default();
    let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    assert_eq!(
        km.action_for(&ctrl('l')),
        Some(Action::ModelSelect),
        "core/keybindings.ts:85"
    );
    assert_eq!(
        km.action_for(&ctrl('t')),
        Some(Action::ThinkingToggle),
        "core/keybindings.ts:87-90"
    );
    assert_eq!(
        km.action_for(&ctrl('x')),
        Some(Action::MessageCopy),
        "core/keybindings.ts:99-102"
    );
    for unbound in [
        Action::SessionNew,
        Action::SessionTree,
        Action::SessionFork,
        Action::SessionResume,
    ] {
        assert!(
            km.keys_label(unbound).is_none(),
            "`defaultKeys: []` (core/keybindings.ts:115-118) — {unbound:?} must ship unbound"
        );
    }
}

/// **TUI-008, the point of the item.** The failure it describes is a *config* failure: the user
/// writes the id upstream documents and nothing happens. Drive the real `merge_json` path.
#[test]
fn tui008_a_keybindings_json_naming_the_new_ids_actually_rebinds_them() {
    let mut km = Keymap::default();
    km.merge_json(
        r#"{"app.session.tree": "ctrl+alt+t", "app.model.select": ["ctrl+alt+m", "f5"]}"#,
    )
    .expect("valid document");
    let ctrl_alt = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL | KeyModifiers::ALT);
    assert_eq!(
        km.action_for(&ctrl_alt('t')),
        Some(Action::SessionTree),
        "a `defaultKeys: []` id is bindable"
    );
    assert_eq!(km.action_for(&ctrl_alt('m')), Some(Action::ModelSelect));
    assert_eq!(
        km.action_for(&KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
        Some(Action::ModelSelect),
        "the array form binds every key in it — and `f5` is a real upstream `SpecialKey` \
         (`tui/src/keys.ts:128-139`), which `Key::parse` used to reject outright"
    );
    // The rebind REPLACES the default (`packages/tui/src/keybindings.ts:187-191` — user keys are
    // not merged with `defaultKeys`), so Ctrl+L no longer opens the selector.
    assert_eq!(
        km.action_for(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)),
        None
    );
}

/// **Found by TUI-008's round-trip test.** `f1`…`f12` and `insert` are upstream `SpecialKey`s
/// (`tui/src/keys.ts:118`, `:128-139` @v0.83.0) with sequence tables at `:380`/`:456-476` and
/// `matchesKey` arms at `:1128-1139`. cyrup's `Key::parse` had no arm for a multi-character token
/// other than the ones listed inline, so every one of them hit the `_ => Err(KeySpec)` fallback and
/// the entire `keybindings.json` entry was thrown away.
///
/// FAILS before the fix: `Key::parse("f5")` is `Err`.
#[test]
fn function_keys_and_insert_parse_and_round_trip_through_label() {
    for (spec, code) in [
        ("f1", KeyCode::F(1)),
        ("f5", KeyCode::F(5)),
        ("f12", KeyCode::F(12)),
        ("insert", KeyCode::Insert),
    ] {
        let key = Key::parse(spec).unwrap_or_else(|e| panic!("{spec}: {e}"));
        assert_eq!(key.code, code, "{spec}");
        // A label that does not read back is a label that lies in `/hotkeys`: the old `Debug`
        // fallback rendered `F(5)` as `f(5)`.
        assert_eq!(key.label(), spec, "{spec} must round-trip");
        assert_eq!(Key::parse(&key.label()).unwrap(), key, "{spec}");
    }
    // With modifiers; `ins` is not a pi `KeyId` (TUI-066).
    assert_eq!(
        Key::parse("ctrl+f4").unwrap(),
        Key {
            code: KeyCode::F(4),
            mods: KeyModifiers::CONTROL
        }
    );
    assert!(Key::parse("ins").is_err());
    // MIRROR — the range is real, not a prefix match: `f0` and `f13` have no upstream `KeyId`, and
    // an `f` followed by non-digits is still an ordinary rejected token.
    assert!(Key::parse("f0").is_err(), "no f0 upstream");
    assert!(Key::parse("f13").is_err(), "keys.ts stops at f12");
    assert!(Key::parse("foo").is_err());
    // …and a bare `f` is still the letter f, not a malformed function key.
    assert_eq!(Key::parse("f").unwrap(), Key::plain(KeyCode::Char('f')));
}

/// TUI-068. `app.session.deleteNoninvasive` resolves and ships bound to `ctrl+backspace`
/// (`core/keybindings.ts:177-180` @v0.84.4). It used to be the one `app.session.*` id `from_id`
/// did not know, which made it unbindable AND unbound with no `KeybindingIssue`.
#[test]
fn tui068_session_delete_noninvasive_resolves_and_defaults_to_ctrl_backspace() {
    use crate::keymap::{SessionAction, SessionKeymap};
    assert_eq!(
        SessionAction::from_id("app.session.deleteNoninvasive"),
        Some(SessionAction::DeleteNoninvasive)
    );
    let km = SessionKeymap::default();
    assert_eq!(
        km.keys_label(SessionAction::DeleteNoninvasive).as_deref(),
        Some("ctrl+backspace")
    );
    let ev = KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL);
    assert_eq!(km.action_for(&ev), Some(SessionAction::DeleteNoninvasive));
    // Plain Backspace stays with the search input.
    let plain = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(km.action_for(&plain), None);
}

/// **TUI-073.** `clear` is a real pi `SpecialKey` (`packages/tui/src/keys.ts:119` @v0.85.1) with
/// sequence tables at `:379`/`:399`/`:413`, reverse-lookup rows at `:429-432` (which spell
/// `ctrl+clear` and `shift+clear`) and a `matchesKey` arm at `:990-994` — pi accepts and binds
/// `{"app.interrupt": "clear"}`. crossterm's `KeyCode` has no counterpart, so cyrup cannot bind it;
/// what it used to do was report `invalid key spec: clear`, which is what it also says for a typo.
///
/// FAILS before the fix: clauses 1-3 do not compile (`TuiError::UnsupportedKey` does not exist) and
/// clause 4 asserts the wrong `reason` string.
#[test]
fn tui073_clear_is_rejected_with_an_unsupported_key_diagnostic() {
    use crate::error::TuiError;

    // 1. The bare spelling.
    assert!(
        matches!(Key::parse("clear"), Err(TuiError::UnsupportedKey(ref k)) if k == "clear"),
        "`clear` must report as unsupported, not as unparseable"
    );
    assert_eq!(
        Key::parse("clear").unwrap_err().to_string(),
        "unsupported key \"clear\"",
        "the exact string the user reads"
    );

    // 2. The modified spellings pi's `:431-432` show are real upstream specs. The reported token is
    //    the unsupported KEY, not the whole chord.
    for spec in ["ctrl+clear", "shift+clear"] {
        assert!(
            matches!(Key::parse(spec), Err(TuiError::UnsupportedKey(ref k)) if k == "clear"),
            "{spec} must name `clear`, not the whole spec"
        );
        assert_eq!(
            Key::parse(spec).unwrap_err().to_string(),
            "unsupported key \"clear\"",
            "{spec}"
        );
    }

    // 3. A typo is still a typo — the whole point of the row is that the two are distinguishable.
    assert!(
        matches!(Key::parse("clrea"), Err(TuiError::KeySpec(ref s)) if s == "clrea"),
        "an unknown multi-character token is still an invalid spec"
    );
    assert_eq!(
        Key::parse("clrea").unwrap_err().to_string(),
        "invalid key spec: clrea"
    );
}

/// **TUI-073, the CFG-038 half.** One `clear` entry must not take the document down with it: the
/// key is dropped, that entry's action ends up UNBOUND (pi's never-matching-`KeyId` outcome, not a
/// revert to the default), every other entry applies, and the user gets exactly one issue naming
/// the id and the new diagnostic.
#[test]
fn tui073_a_clear_entry_is_reported_once_and_does_not_break_the_document() {
    let mut km = Keymap::default();
    let issues = km
        .merge_json(r#"{"app.interrupt": "clear", "app.exit": "f7"}"#)
        .expect("the document itself is well-formed JSON");

    assert_eq!(issues.len(), 1, "exactly one rejected key: {issues:?}");
    assert_eq!(issues[0].id, "app.interrupt");
    assert_eq!(issues[0].reason, "unsupported key \"clear\"");

    // The sibling entry landed.
    assert_eq!(
        km.action_for(&KeyEvent::new(KeyCode::F(7), KeyModifiers::NONE)),
        Some(Action::Quit)
    );
    // ...and `app.interrupt` is unbound, NOT back on its `escape` default.
    assert_eq!(
        km.action_for(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        None,
        "a rejected key still replaces the default, exactly as a never-matching KeyId does"
    );
}

// ---------------------------------------------------------------- CFG-064: raw 0x08 on WT --

/// pi `isWindowsTerminalSession()` (`tui/src/keys.ts:715-719` @v0.87.1) over every combination
/// of `WT_SESSION` and the three SSH variables it negates, including JS's empty-string falsiness.
#[test]
fn cfg064_windows_terminal_predicate_is_pis_truth_table() {
    use crate::keymap::is_windows_terminal_session;
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| std::ffi::OsString::from(v))
        }
    };
    assert!(is_windows_terminal_session(env(&[("WT_SESSION", "0b1c")])));
    assert!(!is_windows_terminal_session(env(&[])));
    assert!(
        !is_windows_terminal_session(env(&[("WT_SESSION", "")])),
        "Boolean(\"\") is false"
    );
    for ssh in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"] {
        let pairs: &'static [(&'static str, &'static str)] = match ssh {
            "SSH_CONNECTION" => &[("WT_SESSION", "0b1c"), ("SSH_CONNECTION", "10.0.0.1 22")],
            "SSH_CLIENT" => &[("WT_SESSION", "0b1c"), ("SSH_CLIENT", "10.0.0.1 51000 22")],
            _ => &[("WT_SESSION", "0b1c"), ("SSH_TTY", "/dev/pts/3")],
        };
        assert!(
            !is_windows_terminal_session(env(pairs)),
            "{ssh} set: the session is remote, not Windows Terminal's"
        );
        let empty: &'static [(&'static str, &'static str)] = match ssh {
            "SSH_CONNECTION" => &[("WT_SESSION", "0b1c"), ("SSH_CONNECTION", "")],
            "SSH_CLIENT" => &[("WT_SESSION", "0b1c"), ("SSH_CLIENT", "")],
            _ => &[("WT_SESSION", "0b1c"), ("SSH_TTY", "")],
        };
        assert!(
            is_windows_terminal_session(env(empty)),
            "an empty {ssh} is falsy upstream and must not negate"
        );
        let no_wt: &'static [(&'static str, &'static str)] = match ssh {
            "SSH_CONNECTION" => &[("SSH_CONNECTION", "10.0.0.1 22")],
            "SSH_CLIENT" => &[("SSH_CLIENT", "10.0.0.1 51000 22")],
            _ => &[("SSH_TTY", "/dev/pts/3")],
        };
        assert!(!is_windows_terminal_session(env(no_wt)));
    }
}

/// A legacy terminal's raw `0x08`, as crossterm 0.29.0 decodes it (`parse.rs:106-109`).
fn raw_bs() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)
}

/// **CFG-064, off Windows Terminal.** pi `parseKey("\x08")` is `"backspace"` and
/// `matchesRawBackspace(data, 0)` holds (`keys.ts:730-734`, `:1287` @v0.87.1), so a terminal (or
/// tmux setup) that sends BS for Backspace deletes a character in the editor.
///
/// Red before the fix: crossterm's `Ctrl+H` matched nothing, so the key did nothing at all.
#[test]
fn cfg064_raw_bs_is_backspace_off_windows_terminal() {
    crate::keymap::force_windows_terminal_session(false);
    let mut ed = crate::InputEditor::new();
    for c in "abc".chars() {
        ed.handle_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    ed.handle_key(&raw_bs());
    assert_eq!(
        ed.text(),
        "ab",
        "0x08 must reach tui.editor.deleteCharBackward"
    );
    assert_eq!(
        crate::SessionKeymap::default().action_for(&raw_bs()),
        None,
        "off WT the byte is NOT ctrl+backspace, so /resume's deleteNoninvasive does not fire"
    );
}

/// **CFG-064, on Windows Terminal.** WT sends `0x08` for Ctrl+Backspace, so pi reads the byte as
/// `ctrl+backspace`: `/resume`'s `app.session.deleteNoninvasive` (`ctrl+backspace`) fires, and the
/// editor's `backspace` binding does NOT (`matchesRawBackspace(data, 0)` is false on WT).
#[test]
fn cfg064_raw_bs_is_ctrl_backspace_on_windows_terminal() {
    crate::keymap::force_windows_terminal_session(true);
    assert_eq!(
        crate::SessionKeymap::default().action_for(&raw_bs()),
        Some(crate::SessionAction::DeleteNoninvasive),
    );
    let mut ed = crate::InputEditor::new();
    for c in "abc".chars() {
        ed.handle_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    ed.handle_key(&raw_bs());
    assert_eq!(
        ed.text(),
        "abc",
        "ctrl+backspace is not an editor binding upstream"
    );
}

/// The alias adds a meaning to the byte; it never takes `ctrl+h` away — pi's `rawCtrlChar("h")`
/// still matches `0x08` for a `ctrl+h` binding on both sides of the heuristic (`keys.ts:1167`).
#[test]
fn cfg064_a_ctrl_h_binding_still_matches_the_byte() {
    for wt in [false, true] {
        crate::keymap::force_windows_terminal_session(wt);
        assert!(Key::parse("ctrl+h").unwrap().matches(&raw_bs()), "wt={wt}");
    }
}

/// Under the kitty protocol a real Ctrl+H is `CSI 104;5u` and Ctrl+Backspace is `CSI 127;5u`, so a
/// `Ctrl+H` event is never a raw BS byte and must not alias Backspace.
#[test]
fn cfg064_the_alias_is_off_under_the_kitty_protocol() {
    use crate::keyboard_protocol::{KeyboardProtocol, set_current};
    crate::keymap::force_windows_terminal_session(false);
    set_current(KeyboardProtocol::Kitty);
    let backspace = Key::plain(KeyCode::Backspace).matches(&raw_bs());
    set_current(KeyboardProtocol::Unknown);
    assert!(!backspace);
}

/// `ESC 0x08` — what a legacy terminal sends for Alt+Backspace when its Backspace key emits BS —
/// is `alt+backspace` upstream on every terminal (`keys.ts:936-938`, `:1291` @v0.87.1). crossterm
/// reports it as `Ctrl+Alt+H` (an ESC prefix adds `ALT`, `parse.rs:78-87`), so it reaches
/// `tui.editor.deleteWordBackward` only through the same alias as the bare byte, and only outside
/// the kitty protocol, where a `Ctrl+Alt+H` event is a real Ctrl+Alt+H.
///
/// Red before the fix: the event matched no editor binding, so the chord did nothing.
#[test]
fn cfg064_esc_bs_is_alt_backspace() {
    use crate::keyboard_protocol::{KeyboardProtocol, set_current};
    let esc_bs = KeyEvent::new(
        KeyCode::Char('h'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    for wt in [false, true] {
        crate::keymap::force_windows_terminal_session(wt);
        let mut ed = crate::InputEditor::new();
        for c in "foo bar".chars() {
            ed.handle_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        ed.handle_key(&esc_bs);
        assert_eq!(ed.text(), "foo ", "wt={wt}: deleteWordBackward");
        assert!(
            Key::parse("ctrl+alt+h").unwrap().matches(&esc_bs),
            "an alias, not a rewrite"
        );
        assert!(!Key::plain(KeyCode::Backspace).matches(&esc_bs));
    }
    set_current(KeyboardProtocol::Kitty);
    let alt_backspace = Key::parse("alt+backspace").unwrap().matches(&esc_bs);
    set_current(KeyboardProtocol::Unknown);
    assert!(!alt_backspace, "under kitty the event is a real Ctrl+Alt+H");
}

// ------------------------------------------------ CFG-091: WSL defaults, `app.thinking.save` --

/// An env lookup over a fixed table (missing keys are `None`, like `std::env::var_os`).
fn env_of(
    pairs: &'static [(&'static str, &'static str)],
) -> impl Fn(&str) -> Option<std::ffi::OsString> {
    move |k| {
        pairs
            .iter()
            .find(|(name, _)| *name == k)
            .map(|(_, v)| std::ffi::OsString::from(v))
    }
}

/// pi `useWindowsKeybindings(platform, env)` (`core/keybindings.ts:62-67` @v0.87.1).
#[test]
fn cfg091_use_windows_keybindings_is_win32_or_linux_under_wsl() {
    use crate::KeybindingPlatform as P;
    let wsl = P::detect("linux", env_of(&[("WSL_DISTRO_NAME", "Ubuntu")]));
    assert!(wsl.windows_keybindings && !wsl.win32);
    assert!(
        P::detect("linux", env_of(&[("WSL_INTEROP", "/run/WSL/1_interop")])).windows_keybindings,
        "WSL2 sets WSL_INTEROP even when WSL_DISTRO_NAME is scrubbed"
    );
    assert!(!P::detect("linux", env_of(&[])).windows_keybindings);
    assert!(
        !P::detect("linux", env_of(&[("WSL_DISTRO_NAME", "")])).windows_keybindings,
        "Boolean(\"\") is false"
    );
    assert!(
        !P::detect("macos", env_of(&[("WSL_DISTRO_NAME", "Ubuntu")])).windows_keybindings,
        "the WSL arm is `platform === \"linux\"` only"
    );
    let win = P::detect("windows", env_of(&[]));
    assert!(win.windows_keybindings && win.win32);
}

/// **CFG-091's `Verify`.** Under WSL the app defaults move off the chords Windows Terminal
/// reserves: `app.model.cycleBackward` is `alt+p`, `app.message.followUp` `ctrl+q`,
/// `app.message.dequeue` `alt+q` (`core/keybindings.ts:112-141` @v0.87.1) — and the Linux chords
/// they replace are unbound, not kept alongside.
///
/// Red before the fix: the table had no platform input, so WSL got `shift+ctrl+p` / `alt+enter` /
/// `alt+up`.
#[test]
fn cfg091_wsl_app_defaults_are_the_windows_ones() {
    let wsl = crate::KeybindingPlatform::detect("linux", env_of(&[("WSL_DISTRO_NAME", "Ubuntu")]));
    let km = Keymap::for_platform(wsl);
    let alt = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
    assert_eq!(
        km.keys_label(Action::ModelCycleBackward).as_deref(),
        Some("alt+p")
    );
    assert_eq!(km.action_for(&alt('p')), Some(Action::ModelCycleBackward));
    assert_eq!(
        km.action_for(&KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )),
        None
    );
    assert_eq!(km.keys_label(Action::FollowUp).as_deref(), Some("ctrl+q"));
    assert_eq!(
        km.action_for(&KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)),
        None
    );
    assert_eq!(km.keys_label(Action::Dequeue).as_deref(), Some("alt+q"));
    assert_eq!(km.action_for(&alt('q')), Some(Action::Dequeue));

    let linux = Keymap::for_platform(crate::KeybindingPlatform::detect("linux", env_of(&[])));
    assert_eq!(
        linux.keys_label(Action::ModelCycleBackward).as_deref(),
        Some("ctrl+shift+p")
    );
    assert_eq!(
        linux.keys_label(Action::FollowUp).as_deref(),
        Some("alt+enter")
    );
    assert_eq!(linux.keys_label(Action::Dequeue).as_deref(), Some("alt+up"));
}

/// `tui.editor.undo` is `win32 ? "ctrl+z" : windowsKeybindings ? "alt+z" : "ctrl+-"`
/// (`core/keybindings.ts:77-80` @v0.87.1), resolved through the editor's own table.
#[test]
fn cfg091_wsl_undo_is_alt_z() {
    use crate::{EditorAction, EditorKeymap, KeybindingPlatform};
    let wsl = KeybindingPlatform::detect("linux", env_of(&[("WSL_INTEROP", "/run/WSL/1_interop")]));
    let km = EditorKeymap::for_platform(wsl);
    assert_eq!(km.keys_label(EditorAction::Undo).as_deref(), Some("alt+z"));
    assert_eq!(
        km.action_for(&KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT)),
        Some(EditorAction::Undo),
        "alt+z reaches tui.editor.undo under WSL"
    );
    assert_eq!(
        km.action_for(&KeyEvent::new(KeyCode::Char('-'), KeyModifiers::CONTROL)),
        None,
        "and ctrl+- no longer does"
    );

    let linux = EditorKeymap::for_platform(KeybindingPlatform::detect("linux", env_of(&[])));
    assert_eq!(
        linux.keys_label(EditorAction::Undo).as_deref(),
        Some("ctrl+-")
    );
}

/// `tui.altScreen.previousPrompt` / `nextPrompt` are the bare `ctrl+up` / `ctrl+down` alone under
/// `windowsKeybindings` (`core/keybindings.ts:81-88` @v0.87.1).
#[test]
fn cfg091_wsl_alt_screen_prompt_jumps_drop_the_shifted_chord() {
    use crate::{AltScreenAction, AltScreenKeymap, KeybindingPlatform};
    let wsl = KeybindingPlatform::detect("linux", env_of(&[("WSL_DISTRO_NAME", "Ubuntu")]));
    let km = AltScreenKeymap::for_platform(wsl);
    assert_eq!(
        km.keys_label(AltScreenAction::PreviousPrompt).as_deref(),
        Some("ctrl+up")
    );
    assert_eq!(
        km.keys_label(AltScreenAction::NextPrompt).as_deref(),
        Some("ctrl+down")
    );
    let linux = AltScreenKeymap::for_platform(KeybindingPlatform::detect("linux", env_of(&[])));
    assert_eq!(
        linux.keys_label(AltScreenAction::PreviousPrompt).as_deref(),
        Some("ctrl+shift+up/ctrl+up")
    );
}

/// **CFG-091, `app.thinking.save`.** A real id (`core/keybindings.ts:104-107` @v0.87.1, default
/// `ctrl+s`) that `keybindings.json` can rebind, that the `/thinking` picker's persist check reads
/// (`thinking-selector.ts:131`) and whose label its footer prints (`:97`) — driven through
/// `App::load_keybindings_json` and `App::handle_input` with the picker open.
///
/// Red before the fix: the id resolved nowhere and the picker matched a literal `ctrl+s`, so the
/// rebind was ignored and the footer kept advertising `Ctrl+S`.
#[test]
fn cfg091_app_thinking_save_is_rebindable_and_drives_the_picker() {
    use crate::{App, AppAction, AppCommand, InputEvent, SelectorKind, UiTheme};
    use ratatui::backend::TestBackend;

    let mut app = App::new(TestBackend::new(80, 30), UiTheme::dark()).unwrap();
    assert!(
        app.effective_keybindings()
            .iter()
            .any(|(id, keys)| id == "app.thinking.save" && keys == &["ctrl+s".to_string()]),
        "the stock default is ctrl+s"
    );
    let issues = app
        .load_keybindings_json(r#"{"app.thinking.save":"ctrl+k"}"#)
        .unwrap();
    assert!(issues.is_empty(), "{issues:?}");

    app.open_selector(SelectorKind::Thinking);
    app.draw().unwrap();
    let buf = app.terminal().backend().buffer().clone();
    let text: String = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                .collect::<String>()
                + "\n"
        })
        .collect();
    assert!(
        text.contains("Ctrl+K to set as default"),
        "the footer names the live key:\n{text}"
    );

    let ctrl = |c: char| InputEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    // The old literal no longer persists — it falls to the search input like any other chord.
    assert!(!matches!(
        app.handle_input(&ctrl('s')),
        AppAction::Command(AppCommand::ConfirmSelectionAsDefault { .. })
    ));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Thinking));
    match app.handle_input(&ctrl('k')) {
        AppAction::Command(AppCommand::ConfirmSelectionAsDefault { kind, value }) => {
            assert_eq!(kind, SelectorKind::Thinking);
            assert_eq!(value, "medium");
        }
        other => panic!("ctrl+k must persist the highlighted level, got {other:?}"),
    }
}

// ------------------------------------------------ TUI-071: platform-conditional app defaults --

/// `app.suspend` is `process.platform === "win32" ? [] : "ctrl+z"` (`core/keybindings.ts:96-99`
/// @v0.87.1) — unbound on native Windows only; WSL keeps job control and `ctrl+z`.
#[test]
fn tui071_suspend_is_unbound_on_native_windows_only() {
    use crate::KeybindingPlatform as P;
    let ctrl_z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);
    let win = Keymap::for_platform(P::detect("windows", env_of(&[])));
    assert_eq!(win.action_for(&ctrl_z), None);
    assert_eq!(win.keys_label(Action::Suspend), None);
    for platform in [
        P::detect("linux", env_of(&[])),
        P::detect("linux", env_of(&[("WSL_DISTRO_NAME", "Ubuntu")])),
        P::detect("macos", env_of(&[])),
    ] {
        assert_eq!(
            Keymap::for_platform(platform).action_for(&ctrl_z),
            Some(Action::Suspend),
            "{platform:?}"
        );
    }
}

/// `app.clipboard.pasteImage` is `windowsKeybindings ? "alt+v" : "ctrl+v"` (`:142-145`) — exactly
/// one key, so the other chord is left to the editor / the terminal's own paste.
#[test]
fn tui071_paste_image_binds_one_key_per_platform() {
    use crate::KeybindingPlatform as P;
    let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
    let alt_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);
    let linux = Keymap::for_platform(P::detect("linux", env_of(&[])));
    assert_eq!(
        linux.keys_label(Action::ClipboardPasteImage).as_deref(),
        Some("ctrl+v")
    );
    assert_eq!(linux.action_for(&ctrl_v), Some(Action::ClipboardPasteImage));
    assert_eq!(linux.action_for(&alt_v), None);
    for windows in [
        P::detect("windows", env_of(&[])),
        P::detect("linux", env_of(&[("WSL_INTEROP", "/run/WSL/1_interop")])),
    ] {
        let km = Keymap::for_platform(windows);
        assert_eq!(
            km.keys_label(Action::ClipboardPasteImage).as_deref(),
            Some("alt+v"),
            "{windows:?}"
        );
        assert_eq!(km.action_for(&ctrl_v), None, "{windows:?}");
    }
}

/// `app.tree.foldOrUp` / `unfoldOrDown` bind the same two keys everywhere, alt-first on darwin
/// and ctrl-first elsewhere (`:150-157`) — the first key is the one `/tree`'s help row shows.
#[test]
fn tui071_tree_fold_keys_are_ctrl_first_off_darwin() {
    use crate::{KeybindingPlatform as P, TreeAction, TreeKeymap};
    let linux = TreeKeymap::for_platform(P::detect("linux", env_of(&[])));
    let mac = TreeKeymap::for_platform(P::detect("macos", env_of(&[])));
    assert_eq!(
        linux.first_key_label(TreeAction::FoldOrUp).as_deref(),
        Some("ctrl+←")
    );
    assert_eq!(
        linux.first_key_label(TreeAction::UnfoldOrDown).as_deref(),
        Some("ctrl+→")
    );
    assert_eq!(
        mac.first_key_label(TreeAction::FoldOrUp).as_deref(),
        Some("alt+←")
    );
    assert_eq!(
        mac.first_key_label(TreeAction::UnfoldOrDown).as_deref(),
        Some("alt+→")
    );
    for km in [&linux, &mac] {
        for mods in [KeyModifiers::ALT, KeyModifiers::CONTROL] {
            assert_eq!(
                km.action_for(&KeyEvent::new(KeyCode::Left, mods)),
                Some(TreeAction::FoldOrUp)
            );
            assert_eq!(
                km.action_for(&KeyEvent::new(KeyCode::Right, mods)),
                Some(TreeAction::UnfoldOrDown)
            );
        }
    }
}

/// With `app.suspend` off `ctrl+z` on native Windows, `tui.editor.undo` takes its `win32` arm
/// (`core/keybindings.ts:77-80`): `ctrl+z`.
#[test]
fn tui071_native_windows_undo_is_ctrl_z() {
    use crate::{EditorAction, EditorKeymap, KeybindingPlatform};
    let km = EditorKeymap::for_platform(KeybindingPlatform::detect("windows", env_of(&[])));
    assert_eq!(km.keys_label(EditorAction::Undo).as_deref(), Some("ctrl+z"));
    assert_eq!(
        km.action_for(&KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL)),
        Some(EditorAction::Undo)
    );
}

// ------------------------------------------------------- TUI-177: pi 1.1's key-spec grammar --
// `parseKeyId` (`packages/tui/src/keys.ts:820-840` @f1b2e77f5; `test/keys.test.ts:137-143`,
// `:278-279`).

#[test]
fn tui177_a_spec_can_name_the_plus_key() {
    assert_eq!(
        Key::parse("ctrl++").unwrap(),
        Key {
            code: KeyCode::Char('+'),
            mods: KeyModifiers::CONTROL,
        }
    );
    assert_eq!(Key::parse("+").unwrap(), Key::plain(KeyCode::Char('+')));
}

#[test]
fn tui177_malformed_specs_are_rejected() {
    for spec in ["ctrl+ctrl+a", "a+b", "ctrl+shift+k+j", "shift+alt+shift+x"] {
        assert!(
            matches!(Key::parse(spec), Err(crate::TuiError::KeySpec(ref s)) if s == spec),
            "{spec} must be a KeySpec error, got {:?}",
            Key::parse(spec)
        );
    }
    // Regression pin: already an error before TUI-177 and still one.
    assert!(matches!(
        Key::parse("shift+"),
        Err(crate::TuiError::KeySpec(_))
    ));
}

#[test]
fn tui177_ctrl_plus_label_round_trips() {
    for spec in ["ctrl++", "+"] {
        assert_eq!(Key::parse(spec).unwrap().label(), spec);
    }
}

/// The match half: what `input/decode.rs` produces for a Kitty `CSI 61:43;6u` (Ctrl+Shift+`=`
/// reporting the shifted `+`; pi's `keys.test.ts:142-143` sequence) is `Char('+')` + CONTROL with
/// SHIFT cleared, and a `ctrl++` binding matches it. `CSI 43;5u` (a layout where `+` is unshifted)
/// decodes to the same event.
#[test]
fn tui177_a_decoded_ctrl_plus_event_matches_ctrl_plus_plus() {
    use crate::input::decode::{Decoded, decode_seq};
    use ratatui::crossterm::event::Event;
    let spec = Key::parse("ctrl++").unwrap();
    for seq in [&b"\x1b[61:43;6u"[..], &b"\x1b[43;5u"[..]] {
        let Decoded::Event(Event::Key(ev)) = decode_seq(seq) else {
            panic!("{seq:?} decodes to a key");
        };
        assert_eq!(ev.code, KeyCode::Char('+'), "{seq:?}");
        assert_eq!(ev.modifiers, KeyModifiers::CONTROL, "{seq:?}");
        assert!(spec.matches(&ev), "{seq:?} matches ctrl++");
    }
}
