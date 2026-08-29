//! Key matching + the configurable keymap (R-10-018 / R-10-023 / R-10-024; arch-10 §3.7).
//!
//! Components MUST NOT hardcode key checks (R-10-018): they resolve an [`Action`] from the
//! [`Keymap`]. The map is seeded with sensible defaults and is replaceable. [`Key::parse`] accepts
//! the string form (`"ctrl+c"`, `"shift+tab"`) for config files and the ext-UI protocol (R-10-023).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::altscreen::TuiRenderMode;
use crate::error::TuiError;

/// Decode the legacy C0 control bytes `0x1C..=0x1F` into the chords pi names for them — a port of
/// `pi/packages/tui/src/keys.ts:1275-1281` @v0.83.0:
///
/// ```text
/// if (data === "\x1c") return "ctrl+\\";
/// if (data === "\x1d") return "ctrl+]";
/// if (data === "\x1f") return "ctrl+-";
/// if (data === "\x1b\x1c") return "ctrl+alt+\\";
/// if (data === "\x1b\x1d") return "ctrl+alt+]";
/// if (data === "\x1b\x1f") return "ctrl+alt+-";
/// ```
///
/// A terminal without the kitty keyboard protocol sends `Ctrl+-` as the single byte `0x1F` and
/// `Ctrl+]` as `0x1D`. crossterm 0.29.0 decodes that whole range **arithmetically** —
/// `c @ b'\x1C'..=b'\x1F' => KeyCode::Char((c - 0x1C + b'4') as char) + CONTROL`
/// (`src/event/sys/unix/parse.rs:110-113`) — so `0x1F` arrives as `Ctrl+7` and `0x1D` as `Ctrl+5`,
/// matching neither `ctrl+-` nor `ctrl+]`. cyrup bound only the CSI-u spellings, so on Terminal.app,
/// iTerm2's default profile, gnome-terminal and plain xterm `editor.undo` did not exist at all
/// (TUI-053) and char-jump-forward was equally dead. `0x1E` is omitted because pi maps no chord to
/// it.
///
/// Gated on the negotiated protocol: under kitty, `Ctrl+7` is genuinely `Ctrl+7` (the terminal sends
/// `CSI 55;5u`) and pi's byte branch is unreachable, so the alias must be too. Under
/// legacy/un-negotiated, `Char('7') + CONTROL` can only have come from `0x1F`. Returns `None` when
/// nothing needs rewriting.
fn normalize_legacy_control_byte(ev: &KeyEvent) -> Option<KeyEvent> {
    if crate::keyboard_protocol::current() == crate::keyboard_protocol::KeyboardProtocol::Kitty {
        return None;
    }
    if !ev.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let decoded = match ev.code {
        KeyCode::Char('4') => '\\',
        KeyCode::Char('5') => ']',
        KeyCode::Char('7') => '-',
        _ => return None,
    };
    Some(KeyEvent::new(KeyCode::Char(decoded), ev.modifiers))
}

/// One `keybindings.json` entry the merge could not use, named so the caller can report it.
///
/// CFG-038. `id` is the (post-migration) binding id; `reason` is why that entry — or one key spec
/// inside it — was rejected. A document can produce several of these and still apply everything
/// else, which is the whole point: upstream has no all-or-nothing failure mode here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeybindingIssue {
    /// The binding id the rejected value was written against.
    pub id: String,
    /// Human-readable reason, suitable for a `warning:` line.
    pub reason: String,
}

impl std::fmt::Display for KeybindingIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.id, self.reason)
    }
}

/// Coerce one binding-id JSON value to its key-spec strings, or `None` when the value is neither a
/// string nor an array whose every element is a string.
///
/// This is Pi's `toKeybindingsConfig` (`core/keybindings.ts:275-287` @v0.83.0) exactly:
///
/// ```ts
/// if (typeof binding === "string") { config[key] = binding; continue; }
/// if (Array.isArray(binding) && binding.every((entry) => typeof entry === "string")) {
///     config[key] = binding;
/// }
/// ```
///
/// Note the missing `else`: a value of any other shape is **dropped from the config**, and the
/// action then falls back to its default (`packages/tui/src/keybindings.ts:187-191` —
/// `userKeys === undefined ? normalizeKeys(definition.defaultKeys) : …`). One malformed entry never
/// touches any other entry.
fn key_specs(value: &serde_json::Value) -> Option<Vec<&str>> {
    match value {
        serde_json::Value::String(s) => Some(vec![s.as_str()]),
        serde_json::Value::Array(items) => items.iter().map(serde_json::Value::as_str).collect(),
        _ => None,
    }
}

/// Walk a keybindings document once, applying every entry this map recognizes and **collecting**
/// the rejects instead of aborting on the first one — CFG-038.
///
/// **Mechanism note (JS → Rust).** Pi cannot fail an individual entry: `toKeybindingsConfig` drops
/// an off-shape value silently (see [`key_specs`]) and a `KeyId` is just a string, so an
/// unparseable spell like `"ctrl+nope+bad+"` survives into `keysById` and simply never matches
/// anything in `matchesKey` (`packages/tui/src/keybindings.ts:198-204` @v0.83.0) — the action ends
/// up **unbound**, not defaulted, and nothing else in the document is affected. cyrup's `Key` is a
/// parsed type, so the two shapes have to be reproduced explicitly:
///
/// * value not a string / not an array-of-strings ⇒ **skip the entry**, action keeps its default;
/// * a spec string that does not parse ⇒ **drop that key** from the entry's list and apply the
///   rest. A dropped key is behaviourally identical to Pi's never-matching `KeyId`; when it was the
///   only key, `set_action(action, vec![])` leaves the action unbound, which is Pi's outcome too.
///
/// Before this, every `merge_json` did `set_action(action, parse_key_values(&value)?)` **inside**
/// the loop, so one bad spec aborted the merge — after the entries ahead of it in iteration order
/// had already been applied — and the binary then printed `warning: ignoring <path>`, which was
/// false: the file had been half-applied, in an order the user cannot see.
///
/// The whole-document error survives for `keybindings_object` only, which is Pi's `loadRawConfig`
/// returning `undefined` for unparseable JSON or a non-object top level
/// (`core/keybindings.ts:328-336`).
fn merge_entries<A>(
    json: &str,
    from_id: impl Fn(&str) -> Option<A>,
    mut apply: impl FnMut(A, Vec<Key>),
) -> Result<Vec<KeybindingIssue>, TuiError> {
    let mut issues = Vec::new();
    for (id, value) in keybindings_object(json)? {
        // Pi `rebuild()`: `if (!(keybinding in this.definitions)) continue;`
        // (`packages/tui/src/keybindings.ts:172-179`). An id this map does not own is not an error
        // — it belongs to one of the other maps, or to a future release.
        let Some(action) = from_id(&id) else { continue };
        let Some(specs) = key_specs(&value) else {
            issues.push(KeybindingIssue {
                id,
                reason: format!("expected a key string or an array of key strings, got {value}"),
            });
            continue;
        };
        let mut keys = Vec::with_capacity(specs.len());
        for spec in specs {
            match Key::parse(spec) {
                Ok(k) => keys.push(k),
                Err(e) => issues.push(KeybindingIssue {
                    id: id.clone(),
                    reason: e.to_string(),
                }),
            }
        }
        apply(action, keys);
    }
    Ok(issues)
}

/// Parse a keybindings document into the `(id, value)` entries of its top-level JSON object
/// (spec/tui/07 §3.9; `core/keybindings.ts:14-262`), applying Pi's legacy-id rename table on the
/// way. Shared by every map's `merge_json`.
///
/// CFG-048 — Pi applies `migrateKeybindingsConfig` **twice**: once at write time from
/// `runMigrations` (`migrations.ts:312`) and once on EVERY read inside
/// `KeybindingsManager.loadFromFile` (`core/keybindings.ts:366`, reached from `create()` `:348-352`
/// and `reload()` `:354-357`). The read-time application is what makes a legacy id work before the
/// on-disk migration has ever run, after a hand-edit, and for a document that never came from a file
/// at all — so it belongs here, in the one function every `merge_json` shares.
///
/// The migration returns an ordered list because `orderKeybindingsConfig` matters for the bytes
/// written back to disk; it does not matter here, so the entries are collected back into a `Map` and
/// no caller changes. Order-independence is a property of the rename, not an accident: the table is
/// injective and a legacy id whose modern twin is also present is DROPPED
/// (`core/keybindings.ts:302-305`), so no two surviving entries can target one id.
fn keybindings_object(json: &str) -> Result<serde_json::Map<String, serde_json::Value>, TuiError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| TuiError::Keybindings(e.to_string()))?;
    match value {
        serde_json::Value::Object(map) => Ok(cyrup_config::migrate_keybindings_config(&map)
            .0
            .into_iter()
            .collect()),
        other => Err(TuiError::Keybindings(format!("expected a JSON object, got {other}"))),
    }
}

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
    /// Open the editor buffer in `$VISUAL`/`$EDITOR` (Ctrl+G) — `app.editor.external`
    /// (`openExternalEditor`, interactive-mode.ts:3611).
    ExternalEditor,
    /// Cycle the reasoning level in place (Shift+Tab) — `app.thinking.cycle`
    /// (`cycleThinkingLevel`, interactive-mode.ts:3606-3614; `core/keybindings.ts:72-75`). Advances
    /// `off→minimal→low→medium→high` (wrapping) and re-colors the editor rule to the new level.
    ThinkingCycle,
    /// Switch to the next model in the cycle set (Ctrl+P) — `app.model.cycleForward`
    /// (`cycleModel("forward")`, interactive-mode.ts:3617-3632; `core/keybindings.ts:76-79`).
    ModelCycleForward,
    /// Switch to the previous model in the cycle set (Shift+Ctrl+P) — `app.model.cycleBackward`
    /// (`cycleModel("backward")`, interactive-mode.ts:3617-3632; `core/keybindings.ts:80-83`).
    ModelCycleBackward,
    /// Queue the editor text as a follow-up delivered after the turn goes idle (Alt+Enter) —
    /// `app.message.followUp` (`handleFollowUp`, interactive-mode.ts:3554-3585;
    /// `core/keybindings.ts:98-101`). Acts as a plain submit when the session is idle.
    FollowUp,
    /// Restore all queued (steering + follow-up) messages back into the editor (Alt+Up) —
    /// `app.message.dequeue` (`handleDequeue` → `restoreQueuedMessagesToEditor`,
    /// interactive-mode.ts:3587-3594,3852-3871; `core/keybindings.ts:102-105`). Clears both queues and
    /// prepends their text (joined by blank lines) to the current editor buffer; shows a
    /// `No queued messages to restore` status when nothing is queued.
    Dequeue,
    /// Paste a system-clipboard image (Ctrl+V; Windows: Alt+V) — `app.clipboard.pasteImage`
    /// (`handleClipboardImagePaste`, interactive-mode.ts:2537-2557; `core/keybindings.ts:106-109`).
    /// Reads the clipboard image via `arboard`, writes it to a `cyrup-clipboard-<uuid>.png` temp file,
    /// and inserts its PATH as text at the editor cursor (Pi's `insertTextAtCursor(filePath)`,
    /// interactive-mode.ts:2552) — so on submit the path rides the outgoing user message AS TEXT, with
    /// no image content block (the agent loads it on demand; the raster never floods context). Gated on
    /// an image actually being present (Pi `clipboard.hasImage()`); a bare Ctrl+V with no clipboard
    /// image falls through to the editor so normal text behavior is preserved.
    ClipboardPasteImage,
    /// Open the model selector (Ctrl+L) — `app.model.select`
    /// (`onAction("app.model.select", () => this.showModelSelector())`,
    /// interactive-mode.ts:2608; `core/keybindings.ts:85`, default `ctrl+l`). The unfiltered picker,
    /// i.e. exactly what a bare `/model` opens.
    ModelSelect,
    /// Toggle whether reasoning blocks are shown (Ctrl+T) — `app.thinking.toggle`
    /// (`toggleThinkingBlockVisibility`, interactive-mode.ts:2610 → `:3834-3850`;
    /// `core/keybindings.ts:87-90`, default `ctrl+t`). Flips `hideThinkingBlock`, PERSISTS it
    /// (`settingsManager.setHideThinkingBlock`, `:3836`) and shows
    /// `Thinking blocks: hidden|visible` (`:3849`). Distinct from
    /// [`ThinkingCycle`](Self::ThinkingCycle), which changes the reasoning *level* on the model.
    ThinkingToggle,
    /// Copy the last assistant message to the clipboard (Ctrl+X) — `app.message.copy`
    /// (`onAction("app.message.copy", () => void this.handleCopyCommand())`,
    /// interactive-mode.ts:2612; `core/keybindings.ts:99-102`, default `ctrl+x`). The same handler
    /// `/copy` runs.
    MessageCopy,
    /// Start a fresh session — `app.session.new` (`handleClearCommand`,
    /// interactive-mode.ts:2615; `core/keybindings.ts:115`). **`defaultKeys: []`** upstream: the id
    /// exists so a `keybindings.json` can bind it, and nothing is bound out of the box.
    SessionNew,
    /// Open the session tree — `app.session.tree` (`showTreeSelector`,
    /// interactive-mode.ts:2616; `core/keybindings.ts:116`, `defaultKeys: []`).
    SessionTree,
    /// Fork from an earlier user message — `app.session.fork` (`showUserMessageSelector`,
    /// interactive-mode.ts:2617; `core/keybindings.ts:117`, `defaultKeys: []`).
    SessionFork,
    /// Resume a persisted session — `app.session.resume` (`showSessionSelector`,
    /// interactive-mode.ts:2618; `core/keybindings.ts:118`, `defaultKeys: []`).
    SessionResume,
}

impl Action {
    /// Resolve the Pi binding id (`app.exit`, `app.interrupt`, …; `core/keybindings.ts:63-202`) to a
    /// global [`Action`]. `None` for ids that belong to the editor/select maps or are unknown.
    pub fn from_id(id: &str) -> Option<Action> {
        match id {
            "app.exit" => Some(Action::Quit),
            "app.interrupt" => Some(Action::Interrupt),
            "app.clear" => Some(Action::Clear),
            "app.suspend" => Some(Action::Suspend),
            "app.pageUp" => Some(Action::PageUp),
            "app.pageDown" => Some(Action::PageDown),
            "app.tools.expand" => Some(Action::ToolsExpand),
            "app.editor.external" => Some(Action::ExternalEditor),
            "app.thinking.cycle" => Some(Action::ThinkingCycle),
            "app.model.cycleForward" => Some(Action::ModelCycleForward),
            "app.model.cycleBackward" => Some(Action::ModelCycleBackward),
            "app.message.followUp" => Some(Action::FollowUp),
            "app.message.dequeue" => Some(Action::Dequeue),
            "app.clipboard.pasteImage" => Some(Action::ClipboardPasteImage),
            // TUI-008 — the seven ids `interactive-mode.ts:2608-2618` wires that cyrup accepted
            // nowhere, so a `keybindings.json` naming any of them silently did nothing.
            "app.model.select" => Some(Action::ModelSelect),
            "app.thinking.toggle" => Some(Action::ThinkingToggle),
            "app.message.copy" => Some(Action::MessageCopy),
            "app.session.new" => Some(Action::SessionNew),
            "app.session.tree" => Some(Action::SessionTree),
            "app.session.fork" => Some(Action::SessionFork),
            "app.session.resume" => Some(Action::SessionResume),
            _ => None,
        }
    }
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
    /// Move the caret UP one page inside the editor buffer — `tui.editor.pageUp`
    /// (`tui/src/keybindings.ts:89`, handled at `tui/src/components/editor.ts:856` →
    /// `pageScroll(-1)`, `:1857`). A page is `max(5, floor(terminalRows * 0.3))` **visual** lines,
    /// the same window [`crate::app::max_visible_editor_lines`] sizes the editor slot from.
    PageUp,
    /// Move the caret DOWN one page inside the editor buffer — `tui.editor.pageDown`
    /// (`tui/src/keybindings.ts:90`; `editor.ts:860` → `pageScroll(1)`).
    PageDown,
    /// Browse to the PREVIOUS prompt-history entry unconditionally — `tui.editor.historyPrevious`
    /// (`tui/src/keybindings.ts:68-71` @v0.84.1, `defaultKeys: []`), handled at
    /// `tui/src/components/editor.ts:767-771` under the comment "Dedicated history actions always
    /// browse entries instead of moving the cursor": cancel the autocomplete, then
    /// `navigateHistory(-1)` — NOT gated on the caret being at the buffer edge the way Up/Down are.
    /// TUI-035; added in the `v0.83.0..v0.84.1` window (absent from `v0.83.0`'s `keybindings.ts`).
    HistoryPrevious,
    /// The forward half — `tui.editor.historyNext` (`keybindings.ts:72-75`, `editor.ts:772-776`).
    HistoryNext,
    /// Decline the key: the editor does NOT consume it, so it falls through as
    /// [`crate::editor::EditorOutcome::Ignored`] instead of being inserted or acted on. This is
    /// `tui.input.copy` (`tui/src/keybindings.ts:36`; `:146` `defaultKeys: "ctrl+c"`, "Copy
    /// selection"), the one upstream id whose entire handler is the bare `return` at
    /// `tui/src/components/editor.ts:653-655`, under the comment "Ctrl+C - let parent handle
    /// (exit/clear)".
    ///
    /// **TUI-067 — bound to nothing by [`EditorKeymap::default`], deliberately.** Upstream needs the
    /// default `ctrl+c` entry because its editor would otherwise swallow the chord; cyrup's default
    /// editor map binds no `ctrl+c` at all, so the chord already reaches the app tier and a default
    /// here would change nothing. What was missing is the DESTINATION: with no arm in
    /// [`EditorAction::from_id`], [`merge_entries`]' `let Some(action) = from_id(&id) else
    /// { continue }` dropped a user's `tui.input.copy` rebind silently — not even a
    /// [`KeybindingIssue`] — so the id was config-inert. The observable effect of a rebind is that
    /// the editor stops INSERTING the rebound chord and declines it instead.
    PassThrough,
}

impl EditorAction {
    /// Resolve an editor binding id to an [`EditorAction`]. `None` for ids outside the editor map.
    ///
    /// **TUI-028.** The canonical spellings are pi's `tui.editor.*` / `tui.input.*`
    /// (`packages/tui/src/keybindings.ts:9-32` @v0.83.0 — already the spelling at the ported
    /// baseline). cyrup shipped a bare `editor.*` namespace that matches **neither** pi's current
    /// ids nor pi's legacy ones, so all 24 bindings written from either era of pi's documentation
    /// were silently inert — `merge_json` ignores an id it does not recognise, with no error and no
    /// diagnostic.
    ///
    /// The `editor.*` spellings are kept as accepted ALIASES rather than deleted, the way pi keeps
    /// its own legacy names working through `KEYBINDING_NAME_MIGRATIONS`
    /// (`coding-agent/src/core/keybindings.ts:209-269`, applied by `migrateKeybindingsConfig` at
    /// `:289-309`): a `keybindings.json` written against shipped cyrup must not break. pi's legacy
    /// BARE names (`cursorUp`, `pageUp`, `newLine`, …) are accepted here too, since those are what
    /// its migration table maps and a pi user's old file carries them.
    pub fn from_id(id: &str) -> Option<EditorAction> {
        use EditorAction as E;
        Some(match id {
            // Canonical (`keybindings.ts:9-29`).
            "tui.editor.cursorLeft" | "editor.cursorLeft" | "cursorLeft" => E::CursorLeft,
            "tui.editor.cursorRight" | "editor.cursorRight" | "cursorRight" => E::CursorRight,
            "tui.editor.cursorUp" | "editor.cursorUp" | "cursorUp" => E::CursorUp,
            "tui.editor.cursorDown" | "editor.cursorDown" | "cursorDown" => E::CursorDown,
            "tui.editor.cursorWordLeft" | "editor.cursorWordLeft" | "cursorWordLeft" => {
                E::CursorWordLeft
            }
            "tui.editor.cursorWordRight" | "editor.cursorWordRight" | "cursorWordRight" => {
                E::CursorWordRight
            }
            "tui.editor.cursorLineStart" | "editor.cursorLineStart" | "cursorLineStart" => {
                E::CursorLineStart
            }
            "tui.editor.cursorLineEnd" | "editor.cursorLineEnd" | "cursorLineEnd" => {
                E::CursorLineEnd
            }
            "tui.editor.deleteCharBackward"
            | "editor.deleteCharBackward"
            | "deleteCharBackward" => E::DeleteCharBackward,
            "tui.editor.deleteCharForward" | "editor.deleteCharForward" | "deleteCharForward" => {
                E::DeleteCharForward
            }
            "tui.editor.deleteWordBackward"
            | "editor.deleteWordBackward"
            | "deleteWordBackward" => E::DeleteWordBackward,
            "tui.editor.deleteWordForward" | "editor.deleteWordForward" | "deleteWordForward" => {
                E::DeleteWordForward
            }
            "tui.editor.deleteToLineStart" | "editor.deleteToLineStart" | "deleteToLineStart" => {
                E::DeleteToLineStart
            }
            "tui.editor.deleteToLineEnd" | "editor.deleteToLineEnd" | "deleteToLineEnd" => {
                E::DeleteToLineEnd
            }
            "tui.editor.yank" | "editor.yank" | "yank" => E::Yank,
            "tui.editor.yankPop" | "editor.yankPop" | "yankPop" => E::YankPop,
            "tui.editor.undo" | "editor.undo" | "undo" => E::Undo,
            "tui.editor.jumpForward" | "editor.jumpForward" | "jumpForward" => E::JumpForward,
            "tui.editor.jumpBackward" | "editor.jumpBackward" | "jumpBackward" => E::JumpBackward,
            // TUI-028 — `app.pageUp`/`app.pageDown` were cyrup inventions; upstream has neither.
            // Paging inside the editor buffer is `tui.editor.pageUp`/`pageDown`
            // (`keybindings.ts:19-20`). The `app.*` spellings stay as aliases for the same reason
            // the `editor.*` ones do.
            "tui.editor.pageUp" | "editor.pageUp" | "pageUp" => E::PageUp,
            "tui.editor.pageDown" | "editor.pageDown" | "pageDown" => E::PageDown,
            // TUI-035 (`keybindings.ts:11-12`, `:68-75` @v0.84.1) — no cyrup predecessor, so no
            // alias to carry.
            "tui.editor.historyPrevious" => E::HistoryPrevious,
            "tui.editor.historyNext" => E::HistoryNext,
            // `tui.input.*` (`keybindings.ts:30-33`).
            "tui.input.newLine" | "editor.newLine" | "newLine" => E::NewLine,
            "tui.input.submit" | "editor.submit" | "submit" => E::Submit,
            "tui.input.tab" | "editor.tab" | "tab" => E::Tab,
            // TUI-067 (`tui/src/keybindings.ts:36`). The legacy BARE spelling is pi's own `copy`
            // (`coding-agent/src/core/keybindings.ts:260`, mirrored in cyrup's migration table at
            // `cyrup-config/src/keybindings.rs:70`), which is why it is carried here the way
            // `newLine`/`submit`/`tab` carry theirs. There is no `editor.copy` alias: cyrup never
            // shipped a spelling for this id, because it had no destination at all until now.
            "tui.input.copy" | "copy" => E::PassThrough,
            _ => return None,
        })
    }
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

impl SelectAction {
    /// Resolve a `tui.select.*` binding id to a [`SelectAction`] (spec/tui/05 §10).
    pub fn from_id(id: &str) -> Option<SelectAction> {
        match id {
            "tui.select.up" => Some(SelectAction::Up),
            "tui.select.down" => Some(SelectAction::Down),
            "tui.select.confirm" => Some(SelectAction::Confirm),
            "tui.select.cancel" => Some(SelectAction::Cancel),
            "tui.select.pageUp" => Some(SelectAction::PageUp),
            "tui.select.pageDown" => Some(SelectAction::PageDown),
            _ => None,
        }
    }
}

/// The modifier bits Pi's `matchesKey` considers (`shift|ctrl|alt|super`, keys.ts:779). Every other
/// bit crossterm may report — `HYPER`, `META`, and the Caps-Lock/Num-Lock lock mask (Pi `LOCK_MASK`,
/// keys.ts:299) — is stripped before comparison so a lock-key state never defeats a binding.
const SUPPORTED_MODS: KeyModifiers = KeyModifiers::SHIFT
    .union(KeyModifiers::CONTROL)
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SUPER);

/// Pi `normalizeShiftedLetterIdentityCodepoint` (keys.ts:360-366): with `shift` held, an ASCII
/// `A..=Z` collapses to its lowercase codepoint so a `shift+a` spec and a reported `Char('A')`+`SHIFT`
/// event compare equal. Non-letters and unshifted codes pass through unchanged.
fn normalize_shifted_letter(code: KeyCode, mods: KeyModifiers) -> KeyCode {
    if mods.contains(KeyModifiers::SHIFT)
        && let KeyCode::Char(c) = code
        && c.is_ascii_uppercase()
    {
        return KeyCode::Char(c.to_ascii_lowercase());
    }
    code
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
                "backtab" => code = Some(KeyCode::BackTab),
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
                // Upstream `KeyId` spells these `pageUp`/`pageDown` (`tui/src/keys.ts:122-123`);
                // `label()` emits that same camelCase spelling and every token here is lowercased
                // before matching, so both spellings parse and the label round-trips.
                "pageup" | "pgup" => code = Some(KeyCode::PageUp),
                "pagedown" | "pgdn" => code = Some(KeyCode::PageDown),
                // `insert` and `f1`…`f12` are `SpecialKey`s upstream (`tui/src/keys.ts:118`,
                // `:128-139` @v0.83.0), with real sequence tables (`:380`, `:456-476`) and real
                // `matchesKey` arms (`:1128-1139`). cyrup had neither, so the multi-character token
                // fell through to the `_ => Err` arm below and the WHOLE entry was rejected — which
                // meant `{"app.model.select": "f5"}` silently bound nothing. Found by TUI-008's own
                // round-trip test, which used `f9` as an arbitrary second key.
                //
                // `clear` (`keys.ts:119`) is deliberately absent: crossterm's `KeyCode` has no
                // counterpart, so there is nothing to map it to.
                "insert" | "ins" => code = Some(KeyCode::Insert),
                other
                    if other
                        .strip_prefix('f')
                        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())) =>
                {
                    match other.strip_prefix('f').and_then(|d| d.parse::<u8>().ok()) {
                        Some(n @ 1..=12) => code = Some(KeyCode::F(n)),
                        _ => return Err(TuiError::KeySpec(s.to_string())),
                    }
                }
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

    /// Whether a raw `KeyEvent` matches this spec (Pi `matchesKey` parity, `tui/src/keys.ts:640-772`).
    ///
    /// Two Pi normalizations are applied before comparing so bindings match the way Pi's do:
    /// 1. **Lock-mask + unsupported-modifier stripping** (`modifier & ~LOCK_MASK`, keys.ts:361,656): a
    ///    Caps-Lock / Num-Lock chord (or any modifier outside the supported `shift|ctrl|alt|super`
    ///    set — e.g. crossterm's `HYPER`/`META`) must not defeat a binding. Both the event and the
    ///    spec are masked to [`SUPPORTED_MODS`] before the modifier comparison.
    /// 2. **Shifted-letter identity** (`normalizeShiftedLetterIdentityCodepoint`, keys.ts:360-366):
    ///    with `shift` held, an ASCII `A..=Z` normalizes to its lowercase codepoint, so a `shift+a`
    ///    binding matches a terminal that reports `Char('A')` + `SHIFT` (the Kitty/disambiguate path
    ///    this TUI enables), and vice-versa.
    pub fn matches(&self, ev: &KeyEvent) -> bool {
        let ev_mods = ev.modifiers & SUPPORTED_MODS;
        let self_mods = self.mods & SUPPORTED_MODS;
        if ev_mods != self_mods {
            return false;
        }
        normalize_shifted_letter(ev.code, ev_mods) == normalize_shifted_letter(self.code, self_mods)
    }

    /// A short human label for the key (`esc`, `ctrl+c`, `shift+tab`) — the inverse of [`Key::parse`],
    /// used to build status-band hints like `(esc to cancel)` from the live keymap
    /// (`keybinding-hints.ts:12-27`; spec/tui/01 §6.1: the cancel text is never hardcoded).
    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            s.push_str("ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            s.push_str("alt+");
        }
        // TUI-069. `KeyCode::BackTab` IS shift+tab — its base label below already carries the modifier, so a
        // SHIFT flag on the same event must NOT print a second `shift+`. Upstream can never produce
        // the doubled form: `app.thinking.cycle` declares the single key `"shift+tab"`
        // (`coding-agent/src/core/keybindings.ts:73-76` @v0.83.0) and `formatKeys` prints
        // `getKeys(id)` verbatim (`keybinding-hints.ts:29-40`), so `/hotkeys` reads `Shift+Tab`.
        // Without this guard the `BackTab`+SHIFT binding at [`Keymap::default`] labelled itself
        // `shift+shift+tab`, which is not a chord any terminal reports and which [`Key::parse`]
        // reads back as plain `Tab`+SHIFT — a label that does not round-trip.
        if self.mods.contains(KeyModifiers::SHIFT) && self.code != KeyCode::BackTab {
            s.push_str("shift+");
        }
        if self.mods.contains(KeyModifiers::SUPER) {
            s.push_str("cmd+");
        }
        let base = match self.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "enter".to_string(),
            KeyCode::Tab => "tab".to_string(),
            KeyCode::BackTab => "shift+tab".to_string(),
            // Upstream's key id is the full word: `"app.interrupt": { defaultKeys: "escape" }`
            // (v0.84.1 `coding-agent/src/core/keybindings.ts:66`), `"tui.select.cancel": {
            // defaultKeys: ["escape", "ctrl+c"] }` (`tui/src/keybindings.ts:149-152`), and
            // `formatKeyText` (`keybinding-hints.ts:17-27`) never abbreviates — it only splits on
            // `/` and `+` and rewrites `alt`→`option` on darwin. So every hint reads `escape
            // interrupt`, not `esc interrupt`. [`Key::parse`] accepts both spellings.
            KeyCode::Esc => "escape".to_string(),
            KeyCode::Up => "up".to_string(),
            KeyCode::Down => "down".to_string(),
            KeyCode::Left => "left".to_string(),
            KeyCode::Right => "right".to_string(),
            KeyCode::Home => "home".to_string(),
            KeyCode::End => "end".to_string(),
            KeyCode::Backspace => "backspace".to_string(),
            KeyCode::Delete => "delete".to_string(),
            // TUI-070. Upstream's `KeyId` is camelCase — `defaultKeys: "pageUp"` / `"pageDown"`
            // (`tui/src/keybindings.ts:89-90` @v0.83.0) — and `formatKeyPart` upper-cases only the
            // FIRST character (`keybinding-hints.ts:12-15`), so pi's /hotkeys cell reads `PageUp`.
            // The lowercased spelling rendered `Pageup` / `Pagedown` there. `Key::parse` lowercases
            // its tokens before matching (`:504`), so the camelCase label still round-trips.
            KeyCode::PageUp => "pageUp".to_string(),
            KeyCode::PageDown => "pageDown".to_string(),
            KeyCode::Insert => "insert".to_string(),
            // Without this arm the `Debug` fallback below renders `KeyCode::F(9)` as `f(9)`, which
            // `Key::parse` cannot read back — a label that does not round-trip is a label that
            // lies in `/hotkeys` and in every `keyHint`.
            KeyCode::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_lowercase(),
        };
        s.push_str(&base);
        s
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
                (Key::ctrl('g'), Action::ExternalEditor),
                (Key::plain(KeyCode::PageUp), Action::PageUp),
                (Key::plain(KeyCode::PageDown), Action::PageDown),
                // `app.thinking.cycle` (`core/keybindings.ts:72-75`, default `shift+tab`). A terminal
                // reports Shift+Tab three ways depending on the keyboard protocol: the legacy `CSI Z`
                // `BackTab` (with or without a SHIFT flag) and — under this TUI's Kitty
                // DISAMBIGUATE mode — `Tab`+SHIFT. Bind all three so the cycle fires regardless.
                (Key::plain(KeyCode::BackTab), Action::ThinkingCycle),
                (Key { code: KeyCode::BackTab, mods: KeyModifiers::SHIFT }, Action::ThinkingCycle),
                (Key { code: KeyCode::Tab, mods: KeyModifiers::SHIFT }, Action::ThinkingCycle),
                // `app.model.cycleForward` / `cycleBackward` (`core/keybindings.ts:76-83`).
                (Key::ctrl('p'), Action::ModelCycleForward),
                (
                    Key { code: KeyCode::Char('p'), mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT },
                    Action::ModelCycleBackward,
                ),
                // `app.message.followUp` (`core/keybindings.ts:98-101`, default `alt+enter`).
                (Key { code: KeyCode::Enter, mods: KeyModifiers::ALT }, Action::FollowUp),
                // `app.message.dequeue` (`core/keybindings.ts:102-105`, default `alt+up`).
                (Key { code: KeyCode::Up, mods: KeyModifiers::ALT }, Action::Dequeue),
                // `app.clipboard.pasteImage` (`core/keybindings.ts:106-109`): `ctrl+v` everywhere,
                // `alt+v` on Windows. Bind both so muscle memory works on either platform (the read is
                // gated on an image actually being present, so a bare Ctrl+V still falls through to the
                // editor as before).
                (Key::ctrl('v'), Action::ClipboardPasteImage),
                (Key { code: KeyCode::Char('v'), mods: KeyModifiers::ALT }, Action::ClipboardPasteImage),
                // TUI-008. `app.model.select` `ctrl+l` (`core/keybindings.ts:85`),
                // `app.thinking.toggle` `ctrl+t` (`:87-90`), `app.message.copy` `ctrl+x`
                // (`:99-102`).
                (Key::ctrl('l'), Action::ModelSelect),
                (Key::ctrl('t'), Action::ThinkingToggle),
                (Key::ctrl('x'), Action::MessageCopy),
                // `app.session.new` / `.tree` / `.fork` / `.resume` are declared with
                // `defaultKeys: []` (`core/keybindings.ts:115-118`) — bindable, deliberately
                // unbound. They are NOT listed here on purpose: inventing a default cyrup would be
                // a divergence, and `keys_label` returning `None` is upstream's `keys.length === 0`.
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

    /// All keys currently bound to `action` (the reverse of [`action_for`](Self::action_for)).
    pub fn keys_for(&self, action: Action) -> Vec<Key> {
        self.bindings.iter().filter(|(_, a)| *a == action).map(|(k, _)| *k).collect()
    }

    /// The label of the first key bound to `action` (`escape`, `ctrl+c`), or `None` if unbound.
    ///
    /// Prefer [`keys_label`](Self::keys_label) for anything the user READS: upstream's hint helpers
    /// all funnel through `keyText`, which joins every bound key. This first-key form is for callers
    /// that genuinely need one key.
    pub fn key_label(&self, action: Action) -> Option<String> {
        self.bindings.iter().find(|(_, a)| *a == action).map(|(k, _)| k.label())
    }

    /// **All** keys bound to `action`, joined with `/` — Pi's `keyText`, i.e.
    /// `formatKeys(getKeybindings().getKeys(keybinding))` = `formatKeyText(keys.join("/"))`
    /// (`keybinding-hints.ts:29-36`). `None` when the action is unbound (upstream's
    /// `keys.length === 0` → `""`).
    ///
    /// This is what every `hint(…)` / `keyHint(…)` in the startup block and the status band renders
    /// (`interactive-mode.ts:936-946`, `status-indicator.ts:47,78,100`), so a rebind that binds two
    /// keys shows both instead of silently hiding the second. The app-tier twin of
    /// [`SelectKeymap::keys_label`].
    pub fn keys_label(&self, action: Action) -> Option<String> {
        join_key_labels(self.bindings.iter().filter(|(_, a)| *a == action).map(|(k, _)| k))
    }

    /// Rebind `action` to exactly `keys`, dropping any keys it was previously bound to **and** taking
    /// each new key away from whatever other action held it (a key maps to exactly one global action,
    /// matching `core/keybindings.ts` where a rebind moves the key).
    pub fn set_action(&mut self, action: Action, keys: Vec<Key>) {
        self.bindings.retain(|(k, a)| *a != action && !keys.contains(k));
        for key in keys {
            self.bindings.push((key, action));
        }
    }

    /// Merge a JSON keybindings document (spec/tui/07 §3.9; `core/keybindings.ts:14-262`): each
    /// recognized `app.*` id **replaces** that action's key set with the listed key spec(s). Ids for
    /// the editor/select maps (and unknown ids) are ignored here. Only an unparseable or non-object
    /// DOCUMENT is an error; a rejected entry or key spec comes back as a [`KeybindingIssue`] and
    /// every other entry still applies (CFG-038 — see [`merge_entries`]).
    pub fn merge_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        merge_entries(json, Action::from_id, |action, keys| self.set_action(action, keys))
    }
}

/// Join the labels of every key bound to one action, `/`-separated — Pi's `formatKeys`, i.e.
/// `keys.join("/")` over `getKeybindings().getKeys(id)` (`keybinding-hints.ts:29-40`). `None` when
/// the action is unbound (upstream's `keys.length === 0` → `""`).
///
/// **A label already emitted is skipped (TUI-069).** Upstream's key list cannot hold the same chord twice —
/// it is the literal `defaultKeys` array or the user's own list. cyrup's default map instead binds
/// ONE chord under several crossterm spellings, because a terminal reports Shift+Tab three ways
/// depending on the keyboard protocol (`Keymap::default`'s three `Action::ThinkingCycle` entries).
/// That is a transport detail; surfacing it would make `/hotkeys` read `Shift+Tab/Shift+Tab/…`
/// where pi prints one cell.
fn join_key_labels<'a>(keys: impl Iterator<Item = &'a Key>) -> Option<String> {
    let mut labels: Vec<String> = Vec::new();
    for label in keys.map(Key::label) {
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    (!labels.is_empty()).then(|| labels.join("/"))
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

    /// The label of the first key bound to `action` (`esc`, `enter`), or `None` if unbound — the
    /// selector-tier twin of [`Keymap::key_label`]. Drives Pi's `keyHint("tui.select.cancel",
    /// "to cancel")` dialog hints (`keybinding-hints.ts:12-27`) from the LIVE keymap, so a
    /// `keybindings.json` rebind of `tui.select.*` changes the hint text too (spec/tui/05 §10; the
    /// cancel text is never hardcoded).
    pub fn key_label(&self, action: SelectAction) -> Option<String> {
        self.bindings.iter().find(|(_, a)| *a == action).map(|(k, _)| k.label())
    }

    /// **All** keys bound to `action`, joined with `/` — Pi's `keyText`, which is
    /// `formatKeys(getKeybindings().getKeys(keybinding))` = `formatKeyText(keys.join("/"))`
    /// (`keybinding-hints.ts:29-36`). `None` when the action is unbound (upstream's `keys.length ===
    /// 0` → `""`).
    ///
    /// This is what a `keyHint("tui.select.cancel", …)` renders: with the stock bindings
    /// (`tui/src/keybindings.ts:149-152`) it is `escape/ctrl+c`, not just the first key.
    pub fn keys_label(&self, action: SelectAction) -> Option<String> {
        join_key_labels(self.bindings.iter().filter(|(_, a)| *a == action).map(|(k, _)| k))
    }

    /// Rebind `action` to exactly `keys`.
    pub fn set_action(&mut self, action: SelectAction, keys: Vec<Key>) {
        self.bindings.retain(|(_, a)| *a != action);
        for key in keys {
            self.bindings.push((key, action));
        }
    }

    /// Merge a JSON keybindings document, applying only the `tui.select.*` ids (spec/tui/05 §10).
    pub fn merge_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        merge_entries(json, SelectAction::from_id, |action, keys| self.set_action(action, keys))
    }
}

/// The configurable autocomplete-popup actions (item #6; `tui.autocomplete.*`). Pi's autocomplete
/// dropdown navigation was matched inline; cyrup routes it through this table so a `keybindings.json`
/// rebind of the popup keys takes effect (consistent with the `tui.select.*` pattern). Defaults:
/// `↑`/`↓` navigate, `Tab` accepts + keeps editing, `Enter` accepts (submitting for a slash item),
/// `Esc` dismisses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutocompleteAction {
    /// Move the highlight up one row — `tui.autocomplete.previous`.
    Previous,
    /// Move the highlight down one row — `tui.autocomplete.next`.
    Next,
    /// Accept the highlighted item and keep editing — `tui.autocomplete.accept` (`Tab`).
    Accept,
    /// Accept the highlighted item, submitting for a slash item — `tui.autocomplete.acceptSubmit` (`Enter`).
    AcceptSubmit,
    /// Dismiss the popup — `tui.autocomplete.cancel` (`Esc`).
    Cancel,
}

impl AutocompleteAction {
    /// Resolve a popup binding id.
    ///
    /// **TUI-028.** Upstream has no `tui.autocomplete.*` family at all: the popup reuses
    /// `tui.select.up` / `tui.select.down` / `tui.select.confirm` / `tui.select.cancel` and
    /// `tui.input.tab` (`packages/tui/src/components/editor.ts:664-712` @v0.83.0). cyrup routed the
    /// popup through an invented map, so rebinding `tui.select.up` moved selector highlights but
    /// NOT the popup — one user-visible action needing two different config keys.
    ///
    /// pi's ids are now accepted here, so a config written against pi's documentation moves both.
    /// The `tui.autocomplete.*` spellings are kept as aliases for the same
    /// do-not-break-a-shipped-config reason the `editor.*` ones are.
    pub fn from_id(id: &str) -> Option<AutocompleteAction> {
        match id {
            "tui.select.up" | "tui.autocomplete.previous" => Some(AutocompleteAction::Previous),
            "tui.select.down" | "tui.autocomplete.next" => Some(AutocompleteAction::Next),
            "tui.input.tab" | "tui.autocomplete.accept" => Some(AutocompleteAction::Accept),
            "tui.select.confirm" | "tui.autocomplete.acceptSubmit" => {
                Some(AutocompleteAction::AcceptSubmit)
            }
            "tui.select.cancel" | "tui.autocomplete.cancel" => Some(AutocompleteAction::Cancel),
            _ => None,
        }
    }
}

/// The configurable autocomplete-popup binding table (item #6). Defaults mirror the previously-
/// hardcoded keys; multiple keys may bind one action, first match wins.
#[derive(Clone, Debug)]
pub struct AutocompleteKeymap {
    bindings: Vec<(Key, AutocompleteAction)>,
}

impl Default for AutocompleteKeymap {
    fn default() -> Self {
        use AutocompleteAction as A;
        AutocompleteKeymap {
            bindings: vec![
                (Key::plain(KeyCode::Up), A::Previous),
                (Key::plain(KeyCode::Down), A::Next),
                (Key::plain(KeyCode::Tab), A::Accept),
                (Key::plain(KeyCode::Enter), A::AcceptSubmit),
                (Key::plain(KeyCode::Esc), A::Cancel),
            ],
        }
    }
}

impl AutocompleteKeymap {
    /// An empty popup keymap.
    pub fn empty() -> Self {
        AutocompleteKeymap { bindings: Vec::new() }
    }

    /// Bind (or override) a key to a popup action.
    pub fn bind(&mut self, key: Key, action: AutocompleteAction) {
        self.bindings.retain(|(k, _)| *k != key);
        self.bindings.push((key, action));
    }

    /// Resolve the popup action for an event, if any (R-10-018: never compare keys inline).
    pub fn action_for(&self, ev: &KeyEvent) -> Option<AutocompleteAction> {
        self.bindings.iter().find_map(|(key, action)| key.matches(ev).then_some(*action))
    }

    /// Rebind `action` to exactly `keys`.
    pub fn set_action(&mut self, action: AutocompleteAction, keys: Vec<Key>) {
        self.bindings.retain(|(_, a)| *a != action);
        for key in keys {
            self.bindings.push((key, action));
        }
    }

    /// Merge a JSON keybindings document, applying only the `tui.autocomplete.*` ids (item #6).
    pub fn merge_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        merge_entries(json, AutocompleteAction::from_id, |action, keys| self.set_action(action, keys))
    }
}

/// The scoped-models checkbox-selector actions (`scoped-models-selector.ts:255-330`;
/// `core/keybindings.ts:150-175` `app.models.*`). These bind only inside the scoped-models selector,
/// on top of the shared `tui.select.*` navigation; resolved via [`ModelsKeymap`] (R-10-018).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelsAction {
    /// Move the highlighted (enabled) model up in cycle order — `app.models.reorderUp` (Alt+Up).
    ReorderUp,
    /// Move the highlighted (enabled) model down in cycle order — `app.models.reorderDown` (Alt+Down).
    ReorderDown,
    /// Enable every model (the scoped set becomes "all") — `app.models.enableAll` (Ctrl+A).
    EnableAll,
    /// Clear the scoped set (no model enabled) — `app.models.clearAll` (Ctrl+X).
    ClearAll,
    /// Toggle every model of the highlighted row's provider — `app.models.toggleProvider` (Ctrl+P).
    ToggleProvider,
    /// Confirm + persist the scoped set — `app.models.save` (Ctrl+S).
    Save,
}

impl ModelsAction {
    /// Resolve an `app.models.*` binding id (spec/tui/05; `core/keybindings.ts:150-175`).
    pub fn from_id(id: &str) -> Option<ModelsAction> {
        match id {
            "app.models.reorderUp" => Some(ModelsAction::ReorderUp),
            "app.models.reorderDown" => Some(ModelsAction::ReorderDown),
            "app.models.enableAll" => Some(ModelsAction::EnableAll),
            "app.models.clearAll" => Some(ModelsAction::ClearAll),
            "app.models.toggleProvider" => Some(ModelsAction::ToggleProvider),
            "app.models.save" => Some(ModelsAction::Save),
            _ => None,
        }
    }
}

/// The configurable scoped-models binding table (`core/keybindings.ts:150-175`). Defaults: Alt+Up/Down
/// reorder, Ctrl+A enable-all, Ctrl+X clear-all, Ctrl+P toggle-provider, Ctrl+S save.
#[derive(Clone, Debug)]
pub struct ModelsKeymap {
    bindings: Vec<(Key, ModelsAction)>,
}

impl Default for ModelsKeymap {
    fn default() -> Self {
        use ModelsAction as M;
        ModelsKeymap {
            bindings: vec![
                (Key { code: KeyCode::Up, mods: KeyModifiers::ALT }, M::ReorderUp),
                (Key { code: KeyCode::Down, mods: KeyModifiers::ALT }, M::ReorderDown),
                (Key::ctrl('a'), M::EnableAll),
                (Key::ctrl('x'), M::ClearAll),
                (Key::ctrl('p'), M::ToggleProvider),
                (Key::ctrl('s'), M::Save),
            ],
        }
    }
}

impl ModelsKeymap {
    /// Resolve the scoped-models action for an event, if any (R-10-018).
    pub fn action_for(&self, ev: &KeyEvent) -> Option<ModelsAction> {
        self.bindings.iter().find_map(|(key, action)| key.matches(ev).then_some(*action))
    }

    /// **All** keys bound to `action`, joined with `/` — the `app.models.*` twin of
    /// [`SelectKeymap::keys_label`], i.e. Pi's `keyText("app.models.…")`
    /// (`keybinding-hints.ts:29-36`). `None` when the action is unbound (upstream's
    /// `keys.length === 0` → `""`).
    ///
    /// Read **only** by the `/scoped-models` footer hint
    /// (`scoped-models-selector.ts:197-205`), which is the only place upstream calls `keyText` on
    /// an `app.models.*` id.
    pub fn keys_label(&self, action: ModelsAction) -> Option<String> {
        join_key_labels(self.bindings.iter().filter(|(_, a)| *a == action).map(|(k, _)| k))
    }

    /// Rebind `action` to exactly `keys`.
    pub fn set_action(&mut self, action: ModelsAction, keys: Vec<Key>) {
        self.bindings.retain(|(_, a)| *a != action);
        for key in keys {
            self.bindings.push((key, action));
        }
    }

    /// Merge a JSON keybindings document, applying only the `app.models.*` ids.
    pub fn merge_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        merge_entries(json, ModelsAction::from_id, |action, keys| self.set_action(action, keys))
    }
}

/// The `/resume` session-picker actions (`session-selector.ts:532-637`; `core/keybindings.ts:91-94,
/// 135-154` `app.session.*`). These bind only inside the session selector, on top of the shared
/// `tui.select.*` navigation; resolved via [`SessionKeymap`] (R-10-018).
///
/// The header's second hint row names **every one of them** through `keyHint("app.session.…", …)`
/// (`session-selector.ts:171-179`), so a rebind has to reach the hint text as well as the handler —
/// which is exactly what a hardcoded `"ctrl+s sort · ctrl+n named · …"` string cannot do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionAction {
    /// Cycle threaded → recent → fuzzy — `app.session.toggleSort` (Ctrl+S).
    ToggleSort,
    /// Toggle the all ↔ named filter — `app.session.toggleNamedFilter` (Ctrl+N).
    ToggleNamedFilter,
    /// Ask to delete the highlighted session — `app.session.delete` (Ctrl+D).
    Delete,
    /// Fold the session path into the metadata column — `app.session.togglePath` (Ctrl+P).
    TogglePath,
    /// Rename the highlighted session — `app.session.rename` (Ctrl+R).
    Rename,
}

impl SessionAction {
    /// Resolve an `app.session.*` binding id (`core/keybindings.ts:91-94,135-154`).
    pub fn from_id(id: &str) -> Option<SessionAction> {
        match id {
            "app.session.toggleSort" => Some(SessionAction::ToggleSort),
            "app.session.toggleNamedFilter" => Some(SessionAction::ToggleNamedFilter),
            "app.session.delete" => Some(SessionAction::Delete),
            "app.session.togglePath" => Some(SessionAction::TogglePath),
            "app.session.rename" => Some(SessionAction::Rename),
            _ => None,
        }
    }
}

/// The configurable `/resume` binding table. Defaults are upstream's verbatim
/// (`core/keybindings.ts:91-94` Ctrl+N, `:135-150` Ctrl+P / Ctrl+S / Ctrl+R / Ctrl+D).
#[derive(Clone, Debug)]
pub struct SessionKeymap {
    bindings: Vec<(Key, SessionAction)>,
}

impl Default for SessionKeymap {
    fn default() -> Self {
        use SessionAction as S;
        SessionKeymap {
            bindings: vec![
                (Key::ctrl('s'), S::ToggleSort),
                (Key::ctrl('n'), S::ToggleNamedFilter),
                (Key::ctrl('d'), S::Delete),
                (Key::ctrl('p'), S::TogglePath),
                (Key::ctrl('r'), S::Rename),
            ],
        }
    }
}

impl SessionKeymap {
    /// Resolve the session-picker action for an event, if any (R-10-018).
    pub fn action_for(&self, ev: &KeyEvent) -> Option<SessionAction> {
        self.bindings.iter().find_map(|(key, action)| key.matches(ev).then_some(*action))
    }

    /// **All** keys bound to `action`, joined with `/` — Pi's `keyText("app.session.…")`
    /// (`keybinding-hints.ts:29-36`). `None` when the action is unbound (upstream's
    /// `keys.length === 0` → `""`).
    pub fn keys_label(&self, action: SessionAction) -> Option<String> {
        join_key_labels(self.bindings.iter().filter(|(_, a)| *a == action).map(|(k, _)| k))
    }

    /// Rebind `action` to exactly `keys`.
    pub fn set_action(&mut self, action: SessionAction, keys: Vec<Key>) {
        self.bindings.retain(|(_, a)| *a != action);
        for key in keys {
            self.bindings.push((key, action));
        }
    }

    /// Merge a JSON keybindings document, applying only the `app.session.*` ids.
    pub fn merge_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        merge_entries(json, SessionAction::from_id, |action, keys| self.set_action(action, keys))
    }
}

/// The `/tree` session-navigator actions (`tree-selector.ts:1180-1197`; `core/keybindings.ts`
/// `app.tree.*`). These bind only inside the tree selector, on top of the shared `tui.select.*`
/// navigation; resolved via [`TreeKeymap`] (R-10-018, spec/tui/05 §6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TreeAction {
    /// Fold the selected branch's descendants, or move up at a leaf — `app.tree.foldOrUp` (`z`).
    FoldOrUp,
    /// Unfold the selected branch, or move down at a leaf — `app.tree.unfoldOrDown` (`x`).
    UnfoldOrDown,
    /// Begin inline label edit on the selected entry — `app.tree.editLabel` (`shift+l`).
    EditLabel,
    /// Toggle the per-row label-timestamp column — `app.tree.toggleLabelTimestamp` (`shift+t`).
    ToggleLabelTimestamp,
    /// Direct filter: default view — `app.tree.filter.default` (`ctrl+d`).
    FilterDefault,
    /// Toggle filter: hide tool results ↔ default — `app.tree.filter.noTools` (`ctrl+t`).
    FilterNoTools,
    /// Toggle filter: user messages only ↔ default — `app.tree.filter.userOnly` (`ctrl+u`).
    FilterUserOnly,
    /// Toggle filter: labeled entries only ↔ default — `app.tree.filter.labeledOnly` (`ctrl+l`).
    FilterLabeledOnly,
    /// Toggle filter: show everything ↔ default — `app.tree.filter.all` (`ctrl+a`).
    FilterAll,
    /// Cycle the filter forwards — `app.tree.filter.cycleForward` (`ctrl+o`).
    FilterCycleForward,
    /// Cycle the filter backwards — `app.tree.filter.cycleBackward` (`shift+ctrl+o`).
    FilterCycleBackward,
    /// Copy the highlighted entry's full text to the clipboard — `app.message.copy` (`ctrl+x`).
    ///
    /// The one binding `/tree` consumes that is NOT an `app.tree.*` id: pi's `handleInput` tests it
    /// alongside the tree's own ids (`tree-selector.ts:1029-1030` `else if (kb.matches(keyData,
    /// "app.message.copy")) { this.copySelected(); }`), and `TREE_HELP_ITEMS` lists it between the
    /// `branch` and `label` cells (`:1217-1235`).
    ///
    /// It lives here rather than as a hole in the chrome's selector-first key route because
    /// [`TreeKeymap::merge_json`] runs the same `merge_entries` over the same JSON the global
    /// [`Keymap`] does, so a user rebind of `app.message.copy` moves BOTH tables at once.
    Copy,
}

impl TreeAction {
    /// Resolve a binding id the `/tree` selector consumes: the `app.tree.*` family (spec/tui/05
    /// §6.1; pi `core/keybindings.ts:119-134` and `:179-206` for the seven `app.tree.filter.*` ids),
    /// plus the one borrowed id `app.message.copy` (`tree-selector.ts:1029-1030`).
    pub fn from_id(id: &str) -> Option<TreeAction> {
        match id {
            "app.tree.foldOrUp" => Some(TreeAction::FoldOrUp),
            "app.tree.unfoldOrDown" => Some(TreeAction::UnfoldOrDown),
            "app.tree.editLabel" => Some(TreeAction::EditLabel),
            "app.tree.toggleLabelTimestamp" => Some(TreeAction::ToggleLabelTimestamp),
            "app.tree.filter.default" => Some(TreeAction::FilterDefault),
            "app.tree.filter.noTools" => Some(TreeAction::FilterNoTools),
            "app.tree.filter.userOnly" => Some(TreeAction::FilterUserOnly),
            "app.tree.filter.labeledOnly" => Some(TreeAction::FilterLabeledOnly),
            "app.tree.filter.all" => Some(TreeAction::FilterAll),
            "app.tree.filter.cycleForward" => Some(TreeAction::FilterCycleForward),
            "app.tree.filter.cycleBackward" => Some(TreeAction::FilterCycleBackward),
            "app.message.copy" => Some(TreeAction::Copy),
            _ => None,
        }
    }
}

/// The configurable `/tree` binding table (spec/tui/05 §6.1).
///
/// **TUI-027.** The defaults are pi's, read at `v0.83.0`
/// `packages/coding-agent/src/core/keybindings.ts:119-134` (`app.tree.foldOrUp` =
/// `["alt+left","ctrl+left"]`, `app.tree.unfoldOrDown` = `["alt+right","ctrl+right"]`,
/// `app.tree.editLabel` = `shift+l`, `app.tree.toggleLabelTimestamp` = `shift+t`) and `:179-206`
/// (the seven `app.tree.filter.*` ids on ctrl+d / ctrl+t / ctrl+u / ctrl+l / ctrl+a, ctrl+o and
/// shift+ctrl+o). cyrup previously bound the four non-filter actions to the bare characters
/// `z` / `x` / `e` / `t`, which upstream accumulates into `/tree`'s **text search** — so a pi user
/// typing an ordinary word into the picker opened the inline label editor and persisted the rest of
/// the word into the session JSONL as an entry label.
#[derive(Clone, Debug)]
pub struct TreeKeymap {
    bindings: Vec<(Key, TreeAction)>,
}

impl Default for TreeKeymap {
    fn default() -> Self {
        use TreeAction as T;
        let ctrl = |c: char| Key { code: KeyCode::Char(c), mods: KeyModifiers::CONTROL };
        let alt_code = |code: KeyCode| Key { code, mods: KeyModifiers::ALT };
        let ctrl_code = |code: KeyCode| Key { code, mods: KeyModifiers::CONTROL };
        let shift = |c: char| Key { code: KeyCode::Char(c), mods: KeyModifiers::SHIFT };
        TreeKeymap {
            bindings: vec![
                (alt_code(KeyCode::Left), T::FoldOrUp),
                (ctrl_code(KeyCode::Left), T::FoldOrUp),
                (alt_code(KeyCode::Right), T::UnfoldOrDown),
                (ctrl_code(KeyCode::Right), T::UnfoldOrDown),
                (shift('l'), T::EditLabel),
                (shift('t'), T::ToggleLabelTimestamp),
                (ctrl('d'), T::FilterDefault),
                (ctrl('t'), T::FilterNoTools),
                (ctrl('u'), T::FilterUserOnly),
                (ctrl('l'), T::FilterLabeledOnly),
                (ctrl('a'), T::FilterAll),
                (ctrl('o'), T::FilterCycleForward),
                (
                    Key {
                        code: KeyCode::Char('o'),
                        mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                    },
                    T::FilterCycleBackward,
                ),
                // `app.message.copy` — the same `ctrl+x` default the global table binds to
                // `Action::MessageCopy` (`core/keybindings.ts:99-102`), so the chord behaves
                // identically whether or not `/tree` owns the input slot. `ctrl+x` collides with
                // none of the chords above.
                (ctrl('x'), T::Copy),
            ],
        }
    }
}

impl TreeKeymap {
    /// Resolve the tree action for an event, if any (R-10-018).
    pub fn action_for(&self, ev: &KeyEvent) -> Option<TreeAction> {
        self.bindings.iter().find_map(|(key, action)| key.matches(ev).then_some(*action))
    }

    /// The label of the **first** key bound to `action`, with pi's help-row arrow substitutions
    /// applied — `formatHelpKeys` (`tree-selector.ts:1238-1253`) takes `getKeys(id)[0]` and rewrites
    /// `up`/`down`/`left`/`right` to `↑`/`↓`/`←`/`→` and `pageUp`/`pageDown` to `pgup`/`pgdn`.
    /// `None` when the action is unbound (upstream's `keys.length === 0` → `""`).
    pub fn first_key_label(&self, action: TreeAction) -> Option<String> {
        let raw = self.bindings.iter().find(|(_, a)| *a == action).map(|(k, _)| k.label())?;
        // pi's replacements are `\b`-anchored, so they rewrite the base key token and never the
        // inside of a longer word. cyrup's labels are `mod+mod+base`, so rewrite the base token.
        let (prefix, base) = match raw.rfind('+') {
            Some(i) => raw.split_at(i + 1),
            None => ("", raw.as_str()),
        };
        let base = match base {
            "pageUp" => "pgup",
            "pageDown" => "pgdn",
            "up" => "↑",
            "down" => "↓",
            "left" => "←",
            "right" => "→",
            other => other,
        };
        Some(format!("{prefix}{base}"))
    }

    /// Rebind `action` to exactly `keys`.
    pub fn set_action(&mut self, action: TreeAction, keys: Vec<Key>) {
        self.bindings.retain(|(_, a)| *a != action);
        for key in keys {
            self.bindings.push((key, action));
        }
    }

    /// Merge a JSON keybindings document, applying only the ids [`TreeAction::from_id`] resolves —
    /// the `app.tree.*` family plus `app.message.copy`. The global [`Keymap`] merges the SAME
    /// document, so a rebind of `app.message.copy` moves this table and the global one together,
    /// which is what keeps the chord identical inside and outside `/tree`.
    pub fn merge_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        merge_entries(json, TreeAction::from_id, |action, keys| self.set_action(action, keys))
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
        use KeyCode::{
            Backspace, Char, Delete, Down, End, Enter, Home, Left, PageDown, PageUp, Right, Tab, Up,
        };
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
                // `ctrl+home` / `ctrl+end` joined the line-start/line-end key sets in **v0.84.1**
                // (`tui/src/keybindings.ts:92-99`: `["home", "ctrl+home", "ctrl+a"]` /
                // `["end", "ctrl+end", "ctrl+e"]`); at the v0.83.0 baseline the sets were
                // `["home","ctrl+a"]` / `["end","ctrl+e"]`. Version lag, not a port bug.
                (ctrl_code(Home), E::CursorLineStart),
                (ctrl('a'), E::CursorLineStart),
                (Key::plain(End), E::CursorLineEnd),
                (ctrl_code(End), E::CursorLineEnd),
                (ctrl('e'), E::CursorLineEnd),
                // Page motion (`keybindings.ts:89-90` at v0.83.0 — `pageUp`/`pageDown` are EDITOR
                // bindings upstream and always have been; pi has no `app.pageUp` at either tag).
                // `ctrl+pageUp`/`ctrl+pageDown` were added to the same sets in v0.84.1
                // (`keybindings.ts:108-109`).
                (Key::plain(PageUp), E::PageUp),
                (ctrl_code(PageUp), E::PageUp),
                (Key::plain(PageDown), E::PageDown),
                (ctrl_code(PageDown), E::PageDown),
                // Deletion + kill ring (`:79-110`).
                (Key::plain(Backspace), E::DeleteCharBackward),
                (Key::plain(Delete), E::DeleteCharForward),
                // Ctrl+D is forward-delete inside the editor (`keybindings.ts` `deleteCharForward`);
                // the global `app.exit` (also Ctrl+D) only fires on an *empty* buffer — the routing
                // guard in `App::handle_input` defers to the editor while text remains (spec/tui/03
                // §6, spec/tui/07 §3.3).
                (ctrl('d'), E::DeleteCharForward),
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
    ///
    /// The event is first put through [`normalize_legacy_control_byte`], pi's `keys.ts:1275-1281`
    /// decoding of the `0x1C..=0x1F` control bytes — without it `editor.undo` is unreachable on any
    /// terminal that does not speak the kitty keyboard protocol (TUI-053).
    pub fn action_for(&self, ev: &KeyEvent) -> Option<EditorAction> {
        let normalized = normalize_legacy_control_byte(ev);
        let ev = normalized.as_ref().unwrap_or(ev);
        self.bindings.iter().find_map(|(key, action)| key.matches(ev).then_some(*action))
    }

    /// The label of the first key bound to `action` (`enter`, `ctrl+w`), or `None` if unbound. Drives
    /// the `/hotkeys` table from the live editor keymap (`getEditorKeyDisplay`, interactive-mode.ts).
    pub fn key_label(&self, action: EditorAction) -> Option<String> {
        self.bindings.iter().find(|(_, a)| *a == action).map(|(k, _)| k.label())
    }

    /// **All** keys bound to `action`, joined with `/` — Pi's `keyText`
    /// (`keybinding-hints.ts:29-36`). The editor-tier twin of [`Keymap::keys_label`] /
    /// [`SelectKeymap::keys_label`]; a hint row that names an editor binding must show every bound
    /// key, e.g. `tui.input.newLine`'s stock `["shift+enter", "ctrl+j"]`
    /// (pi `tui/src/keybindings.ts:137`) renders as `shift+enter/ctrl+j`.
    pub fn keys_label(&self, action: EditorAction) -> Option<String> {
        join_key_labels(self.bindings.iter().filter(|(_, a)| *a == action).map(|(k, _)| k))
    }

    /// **All** keys bound to `action` — the key set behind [`Self::keys_label`], handed out so a
    /// component that owns the input slot can ask "was this an editor binding?" for ONE id without
    /// running the whole editor table over the event.
    ///
    /// This is what pi's `kb.matches(keyData, "tui.input.tab")` is inside a selector
    /// (`session-selector.ts:551` @v0.83.0): the session picker resolves that single editor-tier id
    /// while every other editor binding stays inert, because upstream asks per-id rather than
    /// resolving an event against a table. Calling [`Self::action_for`] there instead would let
    /// `tui.editor.cursorUp` and friends fire inside a list selector, which upstream never does.
    pub fn keys_for(&self, action: EditorAction) -> Vec<Key> {
        self.bindings.iter().filter(|(_, a)| *a == action).map(|(k, _)| *k).collect()
    }

    /// Rebind `action` to exactly `keys`.
    pub fn set_action(&mut self, action: EditorAction, keys: Vec<Key>) {
        self.bindings.retain(|(_, a)| *a != action);
        for key in keys {
            self.bindings.push((key, action));
        }
    }

    /// Merge a JSON keybindings document, applying only the `editor.*` ids (spec/tui/03 §6.1).
    pub fn merge_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        merge_entries(json, EditorAction::from_id, |action, keys| self.set_action(action, keys))
    }
}

/// An alternate-screen viewport action resolved from a key while the fullscreen renderer is live —
/// the eight `tui.altScreen.*` ids of pi's `packages/tui/src/keybindings.ts:44-58` (the interface)
/// and `:159-209` (the definitions) @v0.84.3. ADR-0005 §Decision C, work unit B-9; the ADR's
/// `:44-52` / `:153-179` citations are the @v0.84.1 line numbering for the same two spans.
///
/// Resolved via [`AltScreenKeymap`] so the alternate screen never compares keys inline (R-10-018).
/// What each action *does* to the scroll offset — the page sizes and their floors — is the
/// renderer's half and lives in `altscreen/keys.rs`, over pi's `tui-alt-screen.ts:600-644`.
///
/// # Why this is a map of its own
/// The four unmodified defaults (`pageUp`, `pageDown`, `home`, `end`) are already bound elsewhere:
/// `pageUp`/`pageDown` to [`EditorAction::PageUp`]/[`EditorAction::PageDown`] and to the global
/// [`Action::PageUp`]/[`Action::PageDown`], `home`/`end` to
/// [`EditorAction::CursorLineStart`]/[`EditorAction::CursorLineEnd`]. Upstream's comment at
/// `keybindings.ts:159` — "These intentionally shadow the unmodified editor bindings in fullscreen
/// mode" — is exactly that collision, declared deliberate. Keeping the family in a separate table
/// is what lets the collision be resolved by *mode* ([`AltScreenKeymap::action_in_mode`]) instead
/// of by rebinding anything the inline renderer depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AltScreenAction {
    /// Scroll the viewport up one page — `tui.altScreen.pageUp` (`keybindings.ts:160-163`,
    /// handled at `tui-alt-screen.ts:601-606`). Default `pageUp`.
    PageUp,
    /// Scroll the viewport down one page — `tui.altScreen.pageDown` (`:164-167`; `:607-612`).
    /// Default `pageDown`.
    PageDown,
    /// Scroll the viewport up half a page — `tui.altScreen.halfPageUp` (`:168-171`; `:613-616`).
    /// **`defaultKeys: []`** upstream: the id exists so a `keybindings.json` can bind it, and
    /// nothing is bound out of the box.
    HalfPageUp,
    /// Scroll the viewport down half a page — `tui.altScreen.halfPageDown` (`:172-175`; `:617-620`),
    /// likewise `defaultKeys: []`.
    HalfPageDown,
    /// Jump to the previous semantic prompt — `tui.altScreen.previousPrompt` (`:184-187`;
    /// `:629-632`). The walk itself is ADR-0005 §B-10.
    PreviousPrompt,
    /// Jump to the next semantic prompt — `tui.altScreen.nextPrompt` (`:188-191`; `:633-636`).
    NextPrompt,
    /// Scroll the viewport to its first row — `tui.altScreen.top` (`:208`; `:637-640`). Default
    /// `home`.
    Top,
    /// Scroll the viewport to its last row, re-arming the tail follow — `tui.altScreen.bottom`
    /// (`:209`; `:641-644`). Default `end`.
    Bottom,
}

impl AltScreenAction {
    /// Every viewport action, in pi's declaration order (`keybindings.ts:159-209`) — which is also
    /// the order `handleViewportInput` tests them in (`tui-alt-screen.ts:601-644`).
    ///
    /// A keybindings *surface* enumerates this: pi's registry is a record it can iterate
    /// (`TUI_KEYBINDINGS`, `keybindings.ts:71`), cyrup's is a table keyed by [`Key`], so the list
    /// of ids has to be stated once somewhere. Stating it here is what makes ADR-0005 §Decision C
    /// rule ii checkable — [`AltScreenAction::HalfPageUp`] and [`AltScreenAction::HalfPageDown`]
    /// ship unbound and must still be listed, and an enumeration that skipped them would be the
    /// bug the rule names.
    pub const ALL: [AltScreenAction; 8] = [
        AltScreenAction::PageUp,
        AltScreenAction::PageDown,
        AltScreenAction::HalfPageUp,
        AltScreenAction::HalfPageDown,
        AltScreenAction::PreviousPrompt,
        AltScreenAction::NextPrompt,
        AltScreenAction::Top,
        AltScreenAction::Bottom,
    ];

    /// The binding id this action is written as in a `keybindings.json` — the exact inverse of
    /// [`Self::from_id`], and the string a bindings surface labels the row with.
    pub fn id(self) -> &'static str {
        match self {
            AltScreenAction::PageUp => "tui.altScreen.pageUp",
            AltScreenAction::PageDown => "tui.altScreen.pageDown",
            AltScreenAction::HalfPageUp => "tui.altScreen.halfPageUp",
            AltScreenAction::HalfPageDown => "tui.altScreen.halfPageDown",
            AltScreenAction::PreviousPrompt => "tui.altScreen.previousPrompt",
            AltScreenAction::NextPrompt => "tui.altScreen.nextPrompt",
            AltScreenAction::Top => "tui.altScreen.top",
            AltScreenAction::Bottom => "tui.altScreen.bottom",
        }
    }

    /// The one-line description upstream ships beside the default keys — `KeybindingDefinition`'s
    /// `description` field (`keybindings.ts:63-66`), verbatim from `:160-209`.
    ///
    /// cyrup's other maps carry no descriptions because upstream's `/hotkeys` writes its own prose
    /// per row (`interactive-mode.ts:6284-6294`) and never reads the registry's. These eight are
    /// different only in that no such prose exists to write yet: `/hotkeys` lists no
    /// `tui.altScreen.*` row upstream either, so the string a surface would show is upstream's or
    /// nothing.
    pub fn description(self) -> &'static str {
        match self {
            AltScreenAction::PageUp => "Scroll viewport up one page",
            AltScreenAction::PageDown => "Scroll viewport down one page",
            AltScreenAction::HalfPageUp => "Scroll viewport up half a page",
            AltScreenAction::HalfPageDown => "Scroll viewport down half a page",
            AltScreenAction::PreviousPrompt => "Jump to previous semantic prompt",
            AltScreenAction::NextPrompt => "Jump to next semantic prompt",
            AltScreenAction::Top => "Scroll viewport to top",
            AltScreenAction::Bottom => "Scroll viewport to bottom",
        }
    }

    /// Resolve a `tui.altScreen.*` binding id to an [`AltScreenAction`] (ADR-0005 §Decision C).
    /// `None` for ids that belong to the other maps, or are unknown.
    ///
    /// No aliases: cyrup shipped no predecessor spelling for any of these, so unlike
    /// [`EditorAction::from_id`] there is no legacy name to keep working, and pi's
    /// `KEYBINDING_NAME_MIGRATIONS` (`core/keybindings.ts:209-269`) carries no entry that renames
    /// into this family either.
    ///
    /// **Six ids upstream defines are deliberately not resolved here.**
    /// `tui.altScreen.lineUp`/`lineDown` (`keybindings.ts:176-183`) and the four
    /// `tui.altScreen.search*` ids (`:192-207`) are outside ADR-0005 §Decision C's eight, and
    /// transcript search is not among §Decision B's work units at all — see `altscreen/scroll.rs`,
    /// where the unported search leaves upstream's `activeSearch` permanently unset. Returning
    /// `None` keeps them out of the user's keybindings surface rather than offering a binding whose
    /// handler does not exist; [`merge_entries`] then skips the entry, which is pi's own
    /// `if (!(keybinding in this.definitions)) continue;` (`keybindings.ts:172-179`).
    pub fn from_id(id: &str) -> Option<AltScreenAction> {
        match id {
            "tui.altScreen.pageUp" => Some(AltScreenAction::PageUp),
            "tui.altScreen.pageDown" => Some(AltScreenAction::PageDown),
            "tui.altScreen.halfPageUp" => Some(AltScreenAction::HalfPageUp),
            "tui.altScreen.halfPageDown" => Some(AltScreenAction::HalfPageDown),
            "tui.altScreen.previousPrompt" => Some(AltScreenAction::PreviousPrompt),
            "tui.altScreen.nextPrompt" => Some(AltScreenAction::NextPrompt),
            "tui.altScreen.top" => Some(AltScreenAction::Top),
            "tui.altScreen.bottom" => Some(AltScreenAction::Bottom),
            _ => None,
        }
    }
}

/// The configurable alternate-screen viewport binding table (ADR-0005 §Decision C, work unit B-9).
/// Defaults are pi's verbatim (`packages/tui/src/keybindings.ts:159-209` @v0.84.3); multiple keys
/// may bind one action, first match wins.
///
/// **Every method that resolves an event takes the render mode**, or is documented as the
/// unconditional form the mode-aware one is built from — see [`Self::action_in_mode`]. That is the
/// shadowing rule of `keybindings.ts:159`, and it is the reason this table can bind `pageUp`,
/// `pageDown`, `home` and `end` without disturbing the inline renderer, which resolves those same
/// four chords through [`Keymap`] and [`EditorKeymap`] exactly as it did before ADR-0005.
#[derive(Clone, Debug)]
pub struct AltScreenKeymap {
    bindings: Vec<(Key, AltScreenAction)>,
}

impl Default for AltScreenKeymap {
    /// Upstream's defaults, in upstream's declaration order — which is also the order
    /// `handleViewportInput` tests them in (`tui-alt-screen.ts:601-644`), so
    /// [`Self::action_for`]'s first-match-wins resolution answers what pi's `if` chain answers even
    /// if a user binds one chord to two of these ids.
    ///
    /// [`AltScreenAction::HalfPageUp`] and [`AltScreenAction::HalfPageDown`] are absent on purpose:
    /// upstream declares them `defaultKeys: []` (`keybindings.ts:168-175`), so they are bindable and
    /// deliberately unbound — the same treatment [`Keymap::default`] gives `app.session.new` and
    /// its three siblings. [`Self::keys_label`] returning `None` for them is pi's
    /// `keys.length === 0`.
    ///
    /// `previousPrompt`/`nextPrompt` carry **two** chords each. ADR-0005 §Decision C's table records
    /// the single `ctrl+shift+up` / `ctrl+shift+down` of @v0.84.1; the bare `ctrl+up` / `ctrl+down`
    /// joined those key sets by @v0.84.3 (`keybindings.ts:184-191`), which is the tree this port
    /// reads. Version lag in the table, not a divergence here — the same call
    /// [`EditorKeymap::default`] makes for `ctrl+home`/`ctrl+end`. Neither chord is bound by any
    /// other cyrup map, so nothing is taken away from the editor to add them.
    fn default() -> Self {
        use AltScreenAction as A;
        let ctrl_shift = |code: KeyCode| Key {
            code,
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        };
        let ctrl_code = |code: KeyCode| Key { code, mods: KeyModifiers::CONTROL };
        AltScreenKeymap {
            bindings: vec![
                (Key::plain(KeyCode::PageUp), A::PageUp),
                (Key::plain(KeyCode::PageDown), A::PageDown),
                (ctrl_shift(KeyCode::Up), A::PreviousPrompt),
                (ctrl_code(KeyCode::Up), A::PreviousPrompt),
                (ctrl_shift(KeyCode::Down), A::NextPrompt),
                (ctrl_code(KeyCode::Down), A::NextPrompt),
                (Key::plain(KeyCode::Home), A::Top),
                (Key::plain(KeyCode::End), A::Bottom),
            ],
        }
    }
}

impl AltScreenKeymap {
    /// Resolve the viewport action for an event **unconditionally**, ignoring which renderer is
    /// live (R-10-018).
    ///
    /// This is the raw table lookup. Anything routing real input wants
    /// [`Self::action_in_mode`] instead: calling this from the inline renderer would give
    /// `pageUp`, `pageDown`, `home` and `end` a second meaning there, which is precisely what
    /// ADR-0005 §Decision B forbids. Kept public because a keybindings *surface* — a `/hotkeys`
    /// row, a conflict report — asks "what does this chord mean in fullscreen?" with no live
    /// renderer to ask about.
    pub fn action_for(&self, ev: &KeyEvent) -> Option<AltScreenAction> {
        self.bindings.iter().find_map(|(key, action)| key.matches(ev).then_some(*action))
    }

    /// Resolve the viewport action for an event **only while the fullscreen renderer is live** —
    /// the mode half of the shadowing rule (`keybindings.ts:159`, ADR-0005 §Decision C rule i).
    ///
    /// Under [`TuiRenderMode::Regular`] this answers `None` for every event, including the four
    /// chords the table binds, so an inline session's `pageUp` still reaches
    /// [`EditorAction::PageUp`] and [`Action::PageUp`], and its `home`/`end` still reach
    /// [`EditorAction::CursorLineStart`]/[`EditorAction::CursorLineEnd`]. Under
    /// [`TuiRenderMode::Fullscreen`] it is [`Self::action_for`], and the alternate screen's
    /// dispatcher resolves it ahead of the editor — which is where upstream's precedence comes
    /// from, since it registers the viewport handler as an input listener that runs before the
    /// focused component (`tui-alt-screen.ts:227`, `tui.ts:834-848` against `tui.ts:892-897`).
    ///
    /// The mode is a parameter rather than a field because it is not the table's property: one
    /// keymap outlives any number of ADR-0005 §B-14 mode switches.
    pub fn action_in_mode(&self, ev: &KeyEvent, mode: TuiRenderMode) -> Option<AltScreenAction> {
        match mode {
            TuiRenderMode::Regular => None,
            TuiRenderMode::Fullscreen => self.action_for(ev),
        }
    }

    /// **All** keys bound to `action`, joined with `/` — pi's `keyText`
    /// (`keybinding-hints.ts:29-36`). `None` when the action is unbound, which is upstream's
    /// `keys.length === 0` → `""`.
    ///
    /// This is what puts [`AltScreenAction::HalfPageUp`] and [`AltScreenAction::HalfPageDown`] in
    /// a user-facing bindings list *as unbound*: [`AltScreenAction::ALL`] enumerates them,
    /// [`AltScreenAction::id`] and [`AltScreenAction::description`] name them, and this answers
    /// `None` for their keys until someone binds them (ADR-0005 §Decision C rule ii).
    pub fn keys_label(&self, action: AltScreenAction) -> Option<String> {
        join_key_labels(self.bindings.iter().filter(|(_, a)| *a == action).map(|(k, _)| k))
    }

    /// Rebind `action` to exactly `keys`.
    pub fn set_action(&mut self, action: AltScreenAction, keys: Vec<Key>) {
        self.bindings.retain(|(_, a)| *a != action);
        for key in keys {
            self.bindings.push((key, action));
        }
    }

    /// Merge a JSON keybindings document, applying only the eight `tui.altScreen.*` ids (ADR-0005
    /// §Decision C). Ids belonging to the other maps — and the six upstream ids
    /// [`AltScreenAction::from_id`] deliberately does not resolve — are ignored here, and only an
    /// unparseable or non-object DOCUMENT is an error (CFG-038; see [`merge_entries`]).
    pub fn merge_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        merge_entries(json, AltScreenAction::from_id, |action, keys| {
            self.set_action(action, keys)
        })
    }
}
