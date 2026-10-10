//! `keybindings.schema.json` — port of pi's `KeybindingsSchema`
//! (`packages/coding-agent/src/core/keybindings-schema.ts:1-21` @f1b2e77f5) and `KeyIdSchema`
//! (`core/key-id-schema.ts:1-21`).
//!
//! pi emits one optional property per `KEYBINDINGS` entry, carrying that binding's description,
//! and accepts any other key as a binding too (`additionalProperties: KeybindingValueSchema`) so
//! an extension's own action ids validate. cyrup does the same over
//! [`KEYBINDING_SCHEMA_DESCRIPTIONS`].

use serde_json::{Value, json};

use super::typebox::{array, described, string, union, with};

/// Every keybinding id cyrup resolves, in pi's `KEYBINDINGS` declaration order, with pi's
/// description for it.
///
/// Source: pi's `TUI_KEYBINDINGS` (`packages/tui/src/keybindings.ts` @f1b2e77f5) followed by the
/// coding agent's `app.*` table (`packages/coding-agent/src/core/keybindings.ts` @f1b2e77f5), whose
/// `description` fields `keybindings-schema.ts:11-17` copies into the schema; read back from pi's
/// committed `packages/coding-agent/schemas/keybindings.schema.json` (90 ids) at the same rev.
///
/// **[CYRUP-DELTA] 84 of pi's 90.** It is [`crate::KEYBINDING_IDS`] (74, the ids cyrup's
/// migration and write order know) plus the ten ids cyrup-tui resolves beyond that table:
/// `tui.editor.historyPrevious` / `historyNext` (`EditorAction::from_id`, TUI-035) and the eight
/// `tui.altScreen.*` ids of `AltScreenAction::from_id` (ADR-0005 §Decision C). pi's other six —
/// `tui.altScreen.lineUp` / `lineDown` and the four `tui.altScreen.search*` — are left out exactly
/// as cyrup-tui leaves them out of `from_id` ("keeps them out of the user's keybindings surface
/// rather than offering a binding whose handler does not exist", `keymap.rs`). They still
/// validate as bindings through `additionalProperties`; they simply get no completion entry.
pub const KEYBINDING_SCHEMA_DESCRIPTIONS: [(&str, &str); 84] = [
    // `TUI_KEYBINDINGS` — packages/tui/src/keybindings.ts @f1b2e77f5
    ("tui.editor.cursorUp", "Move cursor up"),
    ("tui.editor.cursorDown", "Move cursor down"),
    (
        "tui.editor.historyPrevious",
        "Select previous prompt history entry",
    ),
    ("tui.editor.historyNext", "Select next prompt history entry"),
    ("tui.editor.cursorLeft", "Move cursor left"),
    ("tui.editor.cursorRight", "Move cursor right"),
    ("tui.editor.cursorWordLeft", "Move cursor word left"),
    ("tui.editor.cursorWordRight", "Move cursor word right"),
    ("tui.editor.cursorLineStart", "Move to line start"),
    ("tui.editor.cursorLineEnd", "Move to line end"),
    ("tui.editor.jumpForward", "Jump forward to character"),
    ("tui.editor.jumpBackward", "Jump backward to character"),
    ("tui.editor.pageUp", "Page up"),
    ("tui.editor.pageDown", "Page down"),
    ("tui.editor.deleteCharBackward", "Delete character backward"),
    ("tui.editor.deleteCharForward", "Delete character forward"),
    ("tui.editor.deleteWordBackward", "Delete word backward"),
    ("tui.editor.deleteWordForward", "Delete word forward"),
    ("tui.editor.deleteToLineStart", "Delete to line start"),
    ("tui.editor.deleteToLineEnd", "Delete to line end"),
    ("tui.editor.yank", "Yank"),
    ("tui.editor.yankPop", "Yank pop"),
    ("tui.editor.undo", "Undo"),
    ("tui.input.newLine", "Insert newline"),
    ("tui.input.submit", "Submit input"),
    ("tui.input.tab", "Tab / autocomplete"),
    ("tui.input.copy", "Copy selection"),
    ("tui.select.up", "Move selection up"),
    ("tui.select.down", "Move selection down"),
    ("tui.select.pageUp", "Selection page up"),
    ("tui.select.pageDown", "Selection page down"),
    ("tui.select.confirm", "Confirm selection"),
    ("tui.select.cancel", "Cancel selection"),
    ("tui.altScreen.pageUp", "Scroll viewport up one page"),
    ("tui.altScreen.pageDown", "Scroll viewport down one page"),
    ("tui.altScreen.halfPageUp", "Scroll viewport up half a page"),
    (
        "tui.altScreen.halfPageDown",
        "Scroll viewport down half a page",
    ),
    (
        "tui.altScreen.previousPrompt",
        "Jump to previous semantic prompt",
    ),
    ("tui.altScreen.nextPrompt", "Jump to next semantic prompt"),
    ("tui.altScreen.top", "Scroll viewport to top"),
    ("tui.altScreen.bottom", "Scroll viewport to bottom"),
    // app ids — packages/coding-agent/src/core/keybindings.ts @f1b2e77f5
    ("app.interrupt", "Cancel or abort"),
    ("app.clear", "Clear editor"),
    ("app.exit", "Exit when editor is empty"),
    ("app.suspend", "Suspend to background"),
    ("app.thinking.cycle", "Cycle thinking level"),
    ("app.thinking.save", "Save thinking level"),
    ("app.model.cycleForward", "Cycle to next model"),
    ("app.model.cycleBackward", "Cycle to previous model"),
    ("app.model.select", "Open model selector"),
    ("app.tools.expand", "Toggle tool output"),
    ("app.thinking.toggle", "Toggle thinking blocks"),
    (
        "app.session.toggleNamedFilter",
        "Toggle named session filter",
    ),
    ("app.editor.external", "Open external editor"),
    (
        "app.message.copy",
        "Copy selection or last assistant message",
    ),
    ("app.message.followUp", "Queue follow-up message"),
    ("app.message.dequeue", "Restore queued messages"),
    (
        "app.clipboard.pasteImage",
        "Paste files on macOS, images, or text from clipboard",
    ),
    ("app.session.new", "Start a new session"),
    ("app.session.tree", "Open session tree"),
    ("app.session.fork", "Fork current session"),
    ("app.session.resume", "Resume a session"),
    ("app.tree.foldOrUp", "Fold tree branch or move up"),
    ("app.tree.unfoldOrDown", "Unfold tree branch or move down"),
    ("app.tree.editLabel", "Edit tree label"),
    (
        "app.tree.toggleLabelTimestamp",
        "Toggle tree label timestamps",
    ),
    ("app.session.togglePath", "Toggle session path display"),
    ("app.session.toggleSort", "Toggle session sort mode"),
    ("app.session.rename", "Rename session"),
    ("app.session.delete", "Delete session"),
    (
        "app.session.deleteNoninvasive",
        "Delete session when query is empty",
    ),
    ("app.models.save", "Save model selection"),
    ("app.models.enableAll", "Enable all models"),
    ("app.models.clearAll", "Clear all models"),
    (
        "app.models.toggleProvider",
        "Toggle all models for provider",
    ),
    ("app.models.reorderUp", "Move model up in order"),
    ("app.models.reorderDown", "Move model down in order"),
    ("app.tree.filter.default", "Tree filter: default view"),
    ("app.tree.filter.noTools", "Tree filter: hide tool results"),
    (
        "app.tree.filter.userOnly",
        "Tree filter: user messages only",
    ),
    (
        "app.tree.filter.labeledOnly",
        "Tree filter: labeled entries only",
    ),
    ("app.tree.filter.all", "Tree filter: show all entries"),
    ("app.tree.filter.cycleForward", "Tree filter: cycle forward"),
    (
        "app.tree.filter.cycleBackward",
        "Tree filter: cycle backward",
    ),
];

/// The string values of pi-tui's `Key` object (`packages/tui/src/keys.ts:163-` @f1b2e77f5, its
/// function-valued members excluded), in declaration order — the base keys `KeyIdSchema` accepts
/// besides a single `[a-z0-9]` (`key-id-schema.ts:5-7`).
const PI_TUI_KEY_VALUES: [&str; 61] = [
    "escape",
    "esc",
    "enter",
    "return",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "clear",
    "home",
    "end",
    "pageUp",
    "pageDown",
    "up",
    "down",
    "left",
    "right",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "`",
    "-",
    "=",
    "[",
    "]",
    "\\",
    ";",
    "'",
    ",",
    ".",
    "/",
    "!",
    "@",
    "#",
    "$",
    "%",
    "^",
    "&",
    "*",
    "(",
    ")",
    "_",
    "+",
    "|",
    "~",
    "{",
    "}",
    ":",
    "<",
    ">",
    "?",
];

/// pi's modifier set (`key-id-schema.ts:4`).
const MODIFIERS: [&str; 4] = ["ctrl", "shift", "alt", "super"];

/// pi's `value.replace(/[\\^$.*+?()[\]{}|]/g, "\\$&")` (`key-id-schema.ts:6`).
fn escape_regex(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if "\\^$.*+?()[]{}|".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `KeyIdSchema`'s pattern (`key-id-schema.ts:5-15`): up to four distinct modifiers, each followed
/// by `+`, then one base key — a lowercase letter or digit, or a pi-tui `Key` name or symbol.
#[must_use]
pub fn key_id_pattern() -> String {
    let base_keys: Vec<String> = PI_TUI_KEY_VALUES.iter().map(|k| escape_regex(k)).collect();
    let base_key_pattern = format!("(?:[a-z0-9]|{})", base_keys.join("|"));
    let duplicate_modifier_pattern = MODIFIERS
        .iter()
        .map(|m| format!("{m}\\+.*{m}\\+"))
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "^(?!.*(?:{duplicate_modifier_pattern}))(?:(?:{})\\+){{0,4}}{base_key_pattern}$",
        MODIFIERS.join("|")
    )
}

/// `KeyIdSchema` (`key-id-schema.ts:10-15`).
fn key_id() -> Value {
    with(
        string(),
        json!({
            "pattern": key_id_pattern(),
            "description": "Key identifier with optional ctrl, shift, alt, or super modifiers.",
        }),
    )
}

/// `keybindingValueSchema(options)` (`key-id-schema.ts:17-19`): one key id or a list of them.
fn keybinding_value() -> Value {
    union(vec![key_id(), array(key_id())])
}

/// The keybindings document's `$defs` (`generate-schemas.ts:51-55`).
pub(super) fn definitions() -> Vec<(&'static str, Value)> {
    vec![("KeybindingValue", keybinding_value())]
}

/// `KeybindingsSchema` (`keybindings-schema.ts:7-21`).
pub(super) fn keybindings_schema() -> Value {
    let mut properties = serde_json::Map::new();
    properties.insert("$schema".into(), super::schema_reference_property(None));
    for (id, description) in KEYBINDING_SCHEMA_DESCRIPTIONS {
        properties.insert(id.into(), described(keybinding_value(), description));
    }
    json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": keybinding_value(),
    })
}
