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
    // With modifiers, and the `ins` alias.
    assert_eq!(
        Key::parse("ctrl+f4").unwrap(),
        Key {
            code: KeyCode::F(4),
            mods: KeyModifiers::CONTROL
        }
    );
    assert_eq!(Key::parse("ins").unwrap().code, KeyCode::Insert);
    // MIRROR — the range is real, not a prefix match: `f0` and `f13` have no upstream `KeyId`, and
    // an `f` followed by non-digits is still an ordinary rejected token.
    assert!(Key::parse("f0").is_err(), "no f0 upstream");
    assert!(Key::parse("f13").is_err(), "keys.ts stops at f12");
    assert!(Key::parse("foo").is_err());
    // …and a bare `f` is still the letter f, not a malformed function key.
    assert_eq!(Key::parse("f").unwrap(), Key::plain(KeyCode::Char('f')));
}
