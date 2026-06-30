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
    /// Quit the interactive session (Ctrl+D on an empty editor) — `app.exit`.
    Quit,
    /// Abort the in-flight run / clear (Esc) — maps to `AgentSession::abort` (R-10-030); `app.interrupt`.
    Interrupt,
    /// Clear the editor buffer (Ctrl+C) — `app.clear`.
    Clear,
    /// Suspend to the background (Ctrl+Z, SIGTSTP) — `app.suspend`.
    Suspend,
    /// Scroll the transcript up a page — `app.pageUp`.
    PageUp,
    /// Scroll the transcript down a page — `app.pageDown`.
    PageDown,
    /// Toggle expansion of the focused tool/bash block (Ctrl+O) — `app.tools.expand`.
    ToolsExpand,
}

/// An editor-level action resolved from a key while the editor owns focus (spec/tui/03 §6.1; the 19
/// editor/input bindings of `pi-tui/src/keybindings.ts:54-134`). Resolved via [`EditorKeymap`] so the
/// editor never compares keys inline (R-10-018).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditorAction {
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
    CursorWordLeft,
    CursorWordRight,
    CursorLineStart,
    CursorLineEnd,
    DeleteCharBackward,
    DeleteCharForward,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    Yank,
    YankPop,
    Undo,
    NewLine,
    Submit,
    Tab,
    JumpForward,
    JumpBackward,
}

/// A selector-level action resolved from a key while a selector owns the input slot (spec/tui/05
/// §10; `tui.select.*`). Resolved via [`SelectKeymap`] so selectors never compare keys inline
/// (R-10-018) — the same hooks Pi's `SelectList.handleInput` reads from `getKeybindings()`
/// (`select-list.ts:98-122`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SelectAction {
    /// Move the highlight up one row (wraps in most selectors) — `tui.select.up`.
    Up,
    /// Move the highlight down one row (wraps) — `tui.select.down`.
    Down,
    /// Confirm the highlighted row — `tui.select.confirm`.
    Confirm,
    /// Dismiss the selector — `tui.select.cancel` (`Esc` / `Ctrl+C`).
    Cancel,
    /// Page the list up — `tui.select.pageUp`.
    PageUp,
    /// Page the list down — `tui.select.pageDown`.
    PageDown,
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

/// A configurable binding table (R-10-018). Pi defaults (`core/keybindings.ts:63-202`): Ctrl+D →
/// exit (`app.exit`), Ctrl+C → clear (`app.clear`), Esc → interrupt, Ctrl+Z → suspend, Ctrl+O →
/// expand, PgUp/PgDn → page.
#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<(Key, Action)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap {
            bindings: vec![
                (Key::ctrl('d'), Action::Quit),
                (Key::ctrl('c'), Action::Clear),
                (Key::plain(KeyCode::Esc), Action::Interrupt),
                (Key::ctrl('z'), Action::Suspend),
                (Key::ctrl('o'), Action::ToolsExpand),
                (Key::plain(KeyCode::PageUp), Action::PageUp),
                (Key::plain(KeyCode::PageDown), Action::PageDown),
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

/// The configurable selector binding table (spec/tui/05 §10). Defaults mirror Pi's `tui.select.*`
/// (`core/keybindings.ts`): `↑`/`↓` move, `Enter` confirms, `Esc`/`Ctrl+C` cancel, `PgUp`/`PgDn`
/// page. Multiple keys may bind one action; first match wins.
#[derive(Clone, Debug)]
pub struct SelectKeymap {
    bindings: Vec<(Key, SelectAction)>,
}

impl Default for SelectKeymap {
    fn default() -> Self {
        use SelectAction as S;
        SelectKeymap {
            bindings: vec![
                (Key::plain(KeyCode::Up), S::Up),
                (Key::plain(KeyCode::Down), S::Down),
                (Key::plain(KeyCode::Enter), S::Confirm),
                (Key::plain(KeyCode::Esc), S::Cancel),
                (Key::ctrl('c'), S::Cancel),
                (Key::plain(KeyCode::PageUp), S::PageUp),
                (Key::plain(KeyCode::PageDown), S::PageDown),
            ],
        }
    }
}

impl SelectKeymap {
    /// An empty selector keymap.
    pub fn empty() -> Self {
        SelectKeymap { bindings: Vec::new() }
    }

    /// Bind (or override) a key to a selector action.
    pub fn bind(&mut self, key: Key, action: SelectAction) {
        self.bindings.retain(|(k, _)| *k != key);
        self.bindings.push((key, action));
    }

    /// Resolve the selector action for an event, if any (R-10-018: never compare keys inline).
    pub fn action_for(&self, ev: &KeyEvent) -> Option<SelectAction> {
        self.bindings.iter().find_map(|(key, action)| key.matches(ev).then_some(*action))
    }
}

/// The configurable editor binding table (spec/tui/03 §6.1). Defaults port
/// `pi-tui/src/keybindings.ts:54-134`. Multiple keys may bind one action; first match wins.
#[derive(Clone, Debug)]
pub struct EditorKeymap {
    bindings: Vec<(Key, EditorAction)>,
}

impl Default for EditorKeymap {
    fn default() -> Self {
        use EditorAction as E;
        use KeyCode::{Backspace, Char, Delete, Down, End, Enter, Home, Left, Right, Tab, Up};
        let ctrl = |c: char| Key { code: Char(c), mods: KeyModifiers::CONTROL };
        let alt = |c: char| Key { code: Char(c), mods: KeyModifiers::ALT };
        let alt_code = |code: KeyCode| Key { code, mods: KeyModifiers::ALT };
        let ctrl_code = |code: KeyCode| Key { code, mods: KeyModifiers::CONTROL };
        EditorKeymap {
            bindings: vec![
                // Motion (`keybindings.ts:54-78`).
                (Key::plain(Left), E::CursorLeft),
                (ctrl('b'), E::CursorLeft),
                (Key::plain(Right), E::CursorRight),
                (ctrl('f'), E::CursorRight),
                (Key::plain(Up), E::CursorUp),
                (Key::plain(Down), E::CursorDown),
                (alt_code(Left), E::CursorWordLeft),
                (ctrl_code(Left), E::CursorWordLeft),
                (alt('b'), E::CursorWordLeft),
                (alt_code(Right), E::CursorWordRight),
                (ctrl_code(Right), E::CursorWordRight),
                (alt('f'), E::CursorWordRight),
                (Key::plain(Home), E::CursorLineStart),
                (ctrl('a'), E::CursorLineStart),
                (Key::plain(End), E::CursorLineEnd),
                (ctrl('e'), E::CursorLineEnd),
                // Deletion + kill ring (`:79-110`).
                (Key::plain(Backspace), E::DeleteCharBackward),
                (Key::plain(Delete), E::DeleteCharForward),
                (ctrl('w'), E::DeleteWordBackward),
                (alt_code(Backspace), E::DeleteWordBackward),
                (alt('d'), E::DeleteWordForward),
                (alt_code(Delete), E::DeleteWordForward),
                (ctrl('u'), E::DeleteToLineStart),
                (ctrl('k'), E::DeleteToLineEnd),
                (ctrl('y'), E::Yank),
                (alt('y'), E::YankPop),
                (ctrl('-'), E::Undo),
                // Char-jump (`:111-114`, Kitty-gated).
                (ctrl(']'), E::JumpForward),
                (Key { code: Char(']'), mods: KeyModifiers::CONTROL | KeyModifiers::ALT }, E::JumpBackward),
                // Newline / submit / tab (`:120-134`).
                (Key { code: Enter, mods: KeyModifiers::SHIFT }, E::NewLine),
                (ctrl('j'), E::NewLine),
                (Key::plain(Enter), E::Submit),
                (Key::plain(Tab), E::Tab),
            ],
        }
    }
}

impl EditorKeymap {
    /// An empty editor keymap.
    pub fn empty() -> Self {
        EditorKeymap { bindings: Vec::new() }
    }

    /// Bind (or override) a key to an editor action.
    pub fn bind(&mut self, key: Key, action: EditorAction) {
        self.bindings.retain(|(k, _)| *k != key);
        self.bindings.push((key, action));
    }

    /// Resolve the editor action for an event, if any.
    pub fn action_for(&self, ev: &KeyEvent) -> Option<EditorAction> {
        self.bindings.iter().find_map(|(key, action)| key.matches(ev).then_some(*action))
    }
}
