//! Key matching + the configurable keymap (R-10-018 / R-10-023 / R-10-024; arch-10 §3.7).
//!
//! Components MUST NOT hardcode key checks (R-10-018): they resolve an [`Action`] from the
//! [`Keymap`]. The map is seeded with sensible defaults and is replaceable. [`Key::parse`] accepts
//! the string form (`"ctrl+c"`, `"shift+tab"`) for config files and the ext-UI protocol (R-10-023).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::error::TuiError;

/// A logical action the chrome reacts to (subset of arch-10 §3.7 `Action`; extended as features
/// land). Editing actions live in the editor itself; these are the *global* bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// Quit the interactive session (Ctrl+D, or Ctrl+C on an empty editor).
    Quit,
    /// Abort the in-flight run / clear (Esc) — maps to `AgentSession::abort` (R-10-030).
    Interrupt,
}

/// A parsed key spec: a base code plus modifiers (R-10-023).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Key {
    /// A Ctrl+char chord.
    pub fn ctrl(c: char) -> Key {
        Key { code: KeyCode::Char(c), mods: KeyModifiers::CONTROL }
    }

    /// A bare key with no modifiers.
    pub fn plain(code: KeyCode) -> Key {
        Key { code, mods: KeyModifiers::NONE }
    }

    /// Parse a string spec like `"ctrl+c"`, `"shift+tab"`, `"alt+enter"`, `"esc"` (R-10-023).
    pub fn parse(s: &str) -> Result<Key, TuiError> {
        let mut mods = KeyModifiers::NONE;
        let mut code: Option<KeyCode> = None;
        for part in s.split('+') {
            let token = part.trim();
            if token.is_empty() {
                continue;
            }
            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
                "shift" => mods |= KeyModifiers::SHIFT,
                "alt" | "option" | "meta" => mods |= KeyModifiers::ALT,
                "super" | "cmd" | "command" => mods |= KeyModifiers::SUPER,
                "enter" | "return" => code = Some(KeyCode::Enter),
                "tab" => code = Some(KeyCode::Tab),
                "esc" | "escape" => code = Some(KeyCode::Esc),
                "space" => code = Some(KeyCode::Char(' ')),
                "up" => code = Some(KeyCode::Up),
                "down" => code = Some(KeyCode::Down),
                "left" => code = Some(KeyCode::Left),
                "right" => code = Some(KeyCode::Right),
                "home" => code = Some(KeyCode::Home),
                "end" => code = Some(KeyCode::End),
                "backspace" => code = Some(KeyCode::Backspace),
                "delete" | "del" => code = Some(KeyCode::Delete),
                other => {
                    let mut chars = other.chars();
                    match (chars.next(), chars.next()) {
                        (Some(c), None) => code = Some(KeyCode::Char(c)),
                        _ => return Err(TuiError::KeySpec(s.to_string())),
                    }
                }
            }
        }
        match code {
            Some(code) => Ok(Key { code, mods }),
            None => Err(TuiError::KeySpec(s.to_string())),
        }
    }

    /// Whether a raw `KeyEvent` matches this spec (pi `matchesKey` parity).
    pub fn matches(&self, ev: &KeyEvent) -> bool {
        ev.code == self.code && ev.modifiers == self.mods
    }
}

/// A configurable binding table (R-10-018). Defaults bind Ctrl+D/Ctrl+C → Quit and Esc → Interrupt.
#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<(Key, Action)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap {
            bindings: vec![
                (Key::ctrl('d'), Action::Quit),
                (Key::ctrl('c'), Action::Quit),
                (Key::plain(KeyCode::Esc), Action::Interrupt),
            ],
        }
    }
}

impl Keymap {
    /// An empty keymap (all keys fall through to the editor).
    pub fn empty() -> Self {
        Keymap { bindings: Vec::new() }
    }

    /// Bind (or override) a key to an action.
    pub fn bind(&mut self, key: Key, action: Action) {
        self.bindings.retain(|(k, _)| *k != key);
        self.bindings.push((key, action));
    }

    /// Resolve the global action for an event, if any (R-10-018: never compare keys inline).
    pub fn action_for(&self, ev: &KeyEvent) -> Option<Action> {
        self.bindings.iter().find_map(|(key, action)| key.matches(ev).then_some(*action))
    }
}
