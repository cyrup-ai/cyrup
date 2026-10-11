//! The one parser of pi's `matchesKey` key-id grammar for overlays, and the one reader of the
//! user's `keybindings.json` they resolve it from (EXT-103).
//!
//! `cyrup-llama`'s `LlamaKeys` and `cyrup-mcp`'s `PanelKeys` each carried a private copy of both
//! (a `KeySpec` parser and a `from_agent_dir` reader), the drift risk `MCP-363a` recorded: the two
//! copies had already diverged on how `shift+tab` meets a `BackTab` keystroke. Neither crate may
//! depend on `cyrup-tui` (whose `Key::parse` has the same token table, over crossterm's types), so
//! the shared home is this module: both already depend on `cyrup-ext`, which owns the
//! [`OverlayKey`] the grammar matches against, and `cyrup-ext` already depends on `cyrup-config`
//! for the legacy-id migration the reader runs. `cyrup-config` could not host the parser (it has no
//! overlay key type and sits below `cyrup-ext`), and `cyrup-tui` is the layer both must avoid.
//!
//! What stays in each overlay is only what is genuinely theirs: which ids they read and their
//! defaults (`Binding` / `LlamaKeys`, `PanelKeys` with the adapter-defined `mcp.panel.save`).

use std::path::Path;

use super::overlay::{OverlayKey, OverlayKeyCode};

/// One `KeyId` spec, parsed from the grammar pi's `matchesKey` accepts (`"ctrl+c"`, `"up"`,
/// `"return"`, `"pageUp"`).
///
/// Shares `cyrup-tui`'s `Key::parse` token table (`crates/cyrup-tui/src/keymap.rs`), including the
/// `enter`/`return` and `esc`/`escape` aliases, over [`OverlayKeyCode`] rather than crossterm's
/// `KeyCode`, but still uses the pre-pi-1.1 grammar: split on every `+`, skip empty tokens, let the
/// last key token win and OR the modifiers together. So `"+"` and `"ctrl++"` name no key here,
/// `"a+b"` parses as `b` and `"ctrl+ctrl+a"` as ctrl+a. pi 1.1's stricter `parseKeyId` grammar
/// (TUI-177) lives in `Key::parse` only. Pi source: `pi/packages/tui/src/keys.ts` `parseKeyId` /
/// `matchesKey`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySpec {
    /// The key itself.
    pub code: OverlayKeyCode,
    /// Control held.
    pub ctrl: bool,
    /// Alt / Meta held.
    pub alt: bool,
    /// Shift explicitly requested by the spec.
    pub shift: bool,
}

impl KeySpec {
    /// Parse one `KeyId`; `None` for a spec naming no key (which never matches, as upstream's
    /// `matchesKey` on an unparseable id).
    #[must_use]
    pub fn parse(spec: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut code: Option<OverlayKeyCode> = None;
        for part in spec.split('+') {
            let token = part.trim();
            if token.is_empty() {
                continue;
            }
            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" | "option" | "meta" => alt = true,
                // No `OverlayKey` bit exists for super, so such a spec can never match.
                "super" | "cmd" | "command" => return None,
                "enter" | "return" => code = Some(OverlayKeyCode::Enter),
                "tab" => code = Some(OverlayKeyCode::Tab),
                "backtab" => code = Some(OverlayKeyCode::BackTab),
                "esc" | "escape" => code = Some(OverlayKeyCode::Escape),
                "space" => code = Some(OverlayKeyCode::Char(' ')),
                "up" => code = Some(OverlayKeyCode::Up),
                "down" => code = Some(OverlayKeyCode::Down),
                "left" => code = Some(OverlayKeyCode::Left),
                "right" => code = Some(OverlayKeyCode::Right),
                "home" => code = Some(OverlayKeyCode::Home),
                "end" => code = Some(OverlayKeyCode::End),
                "backspace" => code = Some(OverlayKeyCode::Backspace),
                "delete" | "del" => code = Some(OverlayKeyCode::Delete),
                "pageup" | "pgup" => code = Some(OverlayKeyCode::PageUp),
                "pagedown" | "pgdn" => code = Some(OverlayKeyCode::PageDown),
                "insert" | "ins" => code = Some(OverlayKeyCode::Insert),
                other => {
                    if let Some(number) = other.strip_prefix('f').filter(|digits| {
                        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                    }) {
                        match number.parse::<u8>() {
                            Ok(n @ 1..=12) => code = Some(OverlayKeyCode::F(n)),
                            _ => return None,
                        }
                        continue;
                    }
                    let mut chars = other.chars();
                    match (chars.next(), chars.next()) {
                        (Some(c), None) => code = Some(OverlayKeyCode::Char(c)),
                        _ => return None,
                    }
                }
            }
        }
        code.map(|code| Self {
            code,
            ctrl,
            alt,
            shift,
        })
    }

    /// Does `key` satisfy this spec?
    ///
    /// Every modifier is compared EXACTLY, as pi's `matchesKey` does
    /// (`pi/packages/tui/src/keys.ts` @f1b2e77f5): each named-key arm accepts only
    /// `modifier === 0`, `MODIFIERS.shift`, `MODIFIERS.alt`, `MODIFIERS.ctrl` or the exact sum the
    /// spec spells, so a bare `up` is not Shift+Up (EXT-106). pi's per-key special cases:
    ///
    /// * `escape` and `f1`..`f12` match only with no modifier at all (`case "escape"`:
    ///   `if (modifier !== 0) return false`; the `f1`..`f12` arm likewise), so `shift+escape` and
    ///   `ctrl+f5` never match.
    /// * A `BackTab` keystroke is `Tab` + shift (`case "tab"`: shift matches `"\x1b[Z"`, no
    ///   modifier matches `"\t"` only), so `"tab"` does not match `BackTab` while `"shift+tab"` and
    ///   `"backtab"` do (EXT-103).
    /// * Characters are matched by [`char_matches`]: the host delivers a printable character
    ///   already shift-resolved (`Shift+k` arrives as `Char('K')`), which pi reads off the raw byte
    ///   the same way.
    #[must_use]
    pub fn matches(&self, key: &OverlayKey) -> bool {
        if self.ctrl != key.ctrl || self.alt != key.alt {
            return false;
        }
        let (want, want_shift) = fold_backtab(self.code, self.shift);
        let (got, got_shift) = fold_backtab(key.code, key.shift);
        match (want, got) {
            (OverlayKeyCode::Escape | OverlayKeyCode::F(_), _)
                if self.ctrl || self.alt || want_shift =>
            {
                false
            }
            (OverlayKeyCode::Char(want), OverlayKeyCode::Char(got)) => {
                char_matches(want, want_shift, got, got_shift)
            }
            (a, b) => a == b && want_shift == got_shift,
        }
    }
}

/// pi's `SYMBOL_KEYS` (`packages/tui/src/keys.ts:258-290` @f1b2e77f5): the punctuation a
/// single-character `KeyId` may name.
const SYMBOL_KEYS: &str = r"`-=[]\;',./!@#$%^&*()_+|~{}:<>?";

/// The character half of [`KeySpec::matches`], pi's single-key tail of `matchesKey`
/// (`keys.ts:1187-1244` @f1b2e77f5). `want` is the spec's (lowercased) character; ctrl and alt
/// were already compared exactly.
///
/// * Space is a named key (`case "space"`): exact.
/// * A letter: an uppercase character IS shift+letter — pi's legacy arm
///   `if (isLetter && data === key.toUpperCase()) return true` under `MODIFIERS.shift`, and
///   `normalizeShiftedLetterIdentityCodepoint` on the Kitty side — so the effective shift is the
///   reported bit OR the case, compared exactly. `"k"` does not match `K`; `"shift+k"` does.
/// * A digit: exact.
/// * A symbol: the produced symbol consumes the shift that made it, ctrl and alt preserved (the
///   "logical match" of `matchesKittySequence` and `matchesPrintableModifyOtherKeys`, which strip
///   `MODIFIERS.shift` from a reported shifted symbol), so `"?"` matches `?` with or without a
///   reported shift; the physical spelling `"shift+?"` still needs the shift.
/// * Anything else never matches: pi's grammar names only a-z, 0-9 and `SYMBOL_KEYS`, and its
///   fall-through is `return false`.
fn char_matches(want: char, want_shift: bool, got: char, got_shift: bool) -> bool {
    if want == ' ' {
        return got == ' ' && want_shift == got_shift;
    }
    if want.is_ascii_lowercase() {
        let shift = got_shift || got.is_ascii_uppercase();
        return got.to_ascii_lowercase() == want && shift == want_shift;
    }
    if want.is_ascii_digit() {
        return got == want && want_shift == got_shift;
    }
    if SYMBOL_KEYS.contains(want) {
        return got == want && (got_shift || !want_shift);
    }
    false
}

/// `BackTab` is `Tab` with shift held.
fn fold_backtab(code: OverlayKeyCode, shift: bool) -> (OverlayKeyCode, bool) {
    match code {
        OverlayKeyCode::BackTab => (OverlayKeyCode::Tab, true),
        other => (other, shift),
    }
}

/// A `KeyId | KeyId[]` value from the user's document, as the list upstream's
/// `Array.isArray(explicit) ? explicit : [explicit]` produces. Non-string array members are
/// skipped; any other JSON type is `None` (as if the id were absent).
#[must_use]
pub fn key_ids(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(one) => Some(vec![one.clone()]),
        serde_json::Value::Array(many) => Some(
            many.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        ),
        _ => None,
    }
}

/// Resolve a `keybindings.json` document's text as `keybindings.getUserBindings()`: the user's raw
/// map with legacy ids migrated by [`cyrup_config::migrate_keybindings_config`] exactly as the TUI
/// does, so no overlay can disagree with the TUI about what `tui.select.up` means.
///
/// `None` for malformed JSON and for a document that is not an object: the caller then resolves
/// from its defaults, pi's "no manager" arm. An empty object is `Some(vec![])`, which also resolves
/// to the defaults but is a real document.
#[must_use]
pub fn parse_user_bindings(raw: &str) -> Option<Vec<(String, serde_json::Value)>> {
    let serde_json::Value::Object(map) = serde_json::from_str::<serde_json::Value>(raw).ok()?
    else {
        return None;
    };
    let (migrated, _) = cyrup_config::migrate_keybindings_config(&map);
    Some(migrated)
}

/// Read `<agent_dir>/keybindings.json` and [`parse_user_bindings`] it. `None` for every failure
/// (absent, unreadable, malformed, not an object).
#[must_use]
pub fn read_user_bindings(agent_dir: &Path) -> Option<Vec<(String, serde_json::Value)>> {
    parse_user_bindings(&std::fs::read_to_string(agent_dir.join("keybindings.json")).ok()?)
}
