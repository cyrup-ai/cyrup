//! Keybindings-config migration — port of Pi's `KEYBINDING_NAME_MIGRATIONS` table and the
//! `migrateKeybindingsConfig` / `migrateKeybindingsConfigFile` pair.
//!
//! Upstream splits this across two files: the table and the pure rewrite live in
//! `pi/packages/coding-agent/src/core/keybindings.ts:209-327` @v0.83.0, and the on-disk startup
//! migration lives in `pi/packages/coding-agent/src/migrations.ts:157-172` @v0.83.0 (called from
//! `runMigrations` at `:312`, between `migrateToolsToBin()` `:311` and `migrateExtensionSystem()`
//! `:313`). Both files are byte-identical at v0.83.0 and v0.84.1, so this is a baseline miss and
//! not upstream drift.
//!
//! **Why it lives in `cyrup-config` and not in the `cyrup` binary.** Pi applies the table
//! **twice**: once at write time from `runMigrations`, and once on **every read** inside
//! `KeybindingsManager.loadFromFile` (`keybindings.ts:363-367`, reached from both
//! `KeybindingsManager.create` `:348-352` and `reload()` `:354-357`). The two cyrup consumers of
//! that pair are `crates/cyrup/src/migrations.rs` (write time) and `crates/cyrup-tui`'s
//! `keymap.rs` (read time), which have no common ancestor other than this crate — the same reason
//! `migrate_settings` (Pi's `migrateSettings`) lives in [`crate::settings`] rather than in the
//! binary. A second copy is exactly what the `encode_cwd` duplication hazard warns against.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value};

/// Legacy → modern keybinding id renames, in Pi's declaration order.
///
/// Port of `KEYBINDING_NAME_MIGRATIONS` (`pi/packages/coding-agent/src/core/keybindings.ts:209-269`
/// @v0.83.0) — 59 entries: 21 → `tui.editor.*`, 4 → `tui.input.*`, 6 → `tui.select.*`,
/// 28 → `app.*`.
///
/// The targets are Pi's **verbatim** modern ids, not a cyrup respelling, so the file that lands on
/// disk is byte-compatible with Pi's. CFG-048's Fix proposed respelling the 25 `tui.editor.*` /
/// `tui.input.*` targets as `editor.*` because it read cyrup as spelling only the latter; that is
/// stale — `EditorAction::from_id` (`crates/cyrup-tui/src/keymap.rs:224-276`) accepts
/// `tui.editor.cursorUp`, `editor.cursorUp` AND the bare legacy `cursorUp`, so Pi's spelling
/// resolves at HEAD and needs no rewrite when TUI-028 lands.
///
/// Not every target has a live binding yet: `tui.input.copy` and the seven `app.*` ids TUI-008
/// tracks have no `from_id` arm, so they migrate correctly and then sit inert until those items
/// land. That is Pi's own outcome for an id it does not define — `rebuild()`
/// (`pi/packages/tui/src/keybindings.ts:243-256` @v0.83.0) skips a binding not in `this.definitions`
/// and costs nothing else.
pub const KEYBINDING_NAME_MIGRATIONS: [(&str, &str); 59] = [
    // 21 → `tui.editor.*` (keybindings.ts:210-230)
    ("cursorUp", "tui.editor.cursorUp"),
    ("cursorDown", "tui.editor.cursorDown"),
    ("cursorLeft", "tui.editor.cursorLeft"),
    ("cursorRight", "tui.editor.cursorRight"),
    ("cursorWordLeft", "tui.editor.cursorWordLeft"),
    ("cursorWordRight", "tui.editor.cursorWordRight"),
    ("cursorLineStart", "tui.editor.cursorLineStart"),
    ("cursorLineEnd", "tui.editor.cursorLineEnd"),
    ("jumpForward", "tui.editor.jumpForward"),
    ("jumpBackward", "tui.editor.jumpBackward"),
    ("pageUp", "tui.editor.pageUp"),
    ("pageDown", "tui.editor.pageDown"),
    ("deleteCharBackward", "tui.editor.deleteCharBackward"),
    ("deleteCharForward", "tui.editor.deleteCharForward"),
    ("deleteWordBackward", "tui.editor.deleteWordBackward"),
    ("deleteWordForward", "tui.editor.deleteWordForward"),
    ("deleteToLineStart", "tui.editor.deleteToLineStart"),
    ("deleteToLineEnd", "tui.editor.deleteToLineEnd"),
    ("yank", "tui.editor.yank"),
    ("yankPop", "tui.editor.yankPop"),
    ("undo", "tui.editor.undo"),
    // 4 → `tui.input.*` (keybindings.ts:231-234)
    ("newLine", "tui.input.newLine"),
    ("submit", "tui.input.submit"),
    ("tab", "tui.input.tab"),
    ("copy", "tui.input.copy"),
    // 6 → `tui.select.*` (keybindings.ts:235-240)
    ("selectUp", "tui.select.up"),
    ("selectDown", "tui.select.down"),
    ("selectPageUp", "tui.select.pageUp"),
    ("selectPageDown", "tui.select.pageDown"),
    ("selectConfirm", "tui.select.confirm"),
    ("selectCancel", "tui.select.cancel"),
    // 28 → `app.*` (keybindings.ts:241-268)
    ("interrupt", "app.interrupt"),
    ("clear", "app.clear"),
    ("exit", "app.exit"),
    ("suspend", "app.suspend"),
    ("cycleThinkingLevel", "app.thinking.cycle"),
    ("cycleModelForward", "app.model.cycleForward"),
    ("cycleModelBackward", "app.model.cycleBackward"),
    ("selectModel", "app.model.select"),
    ("expandTools", "app.tools.expand"),
    ("toggleThinking", "app.thinking.toggle"),
    ("toggleSessionNamedFilter", "app.session.toggleNamedFilter"),
    ("externalEditor", "app.editor.external"),
    ("followUp", "app.message.followUp"),
    ("dequeue", "app.message.dequeue"),
    ("pasteImage", "app.clipboard.pasteImage"),
    ("newSession", "app.session.new"),
    ("tree", "app.session.tree"),
    ("fork", "app.session.fork"),
    ("resume", "app.session.resume"),
    ("treeFoldOrUp", "app.tree.foldOrUp"),
    ("treeUnfoldOrDown", "app.tree.unfoldOrDown"),
    ("treeEditLabel", "app.tree.editLabel"),
    ("treeToggleLabelTimestamp", "app.tree.toggleLabelTimestamp"),
    ("toggleSessionPath", "app.session.togglePath"),
    ("toggleSessionSort", "app.session.toggleSort"),
    ("renameSession", "app.session.rename"),
    ("deleteSession", "app.session.delete"),
    ("deleteSessionNoninvasive", "app.session.deleteNoninvasive"),
];

/// Every declared keybinding id, in Pi's `KEYBINDINGS` **declaration order** — the order
/// `orderKeybindingsConfig` (`keybindings.ts:311-327` @v0.83.0) writes them back in.
///
/// `KEYBINDINGS` is `{ ...TUI_KEYBINDINGS, ...appIds }` (`keybindings.ts:64-65`), so the order is
/// the 31 `tui.*` ids from `pi/packages/tui/src/keybindings.ts:55-131` @v0.83.0 followed by the
/// 42 `app.*` ids from `keybindings.ts:66-207`. An id NOT in this list is an "extra" and is appended
/// in sorted order (`keybindings.ts:320-325`).
pub const KEYBINDING_IDS: [&str; 73] = [
    // `TUI_KEYBINDINGS` — pi/packages/tui/src/keybindings.ts:55-131 @v0.83.0
    "tui.editor.cursorUp",
    "tui.editor.cursorDown",
    "tui.editor.cursorLeft",
    "tui.editor.cursorRight",
    "tui.editor.cursorWordLeft",
    "tui.editor.cursorWordRight",
    "tui.editor.cursorLineStart",
    "tui.editor.cursorLineEnd",
    "tui.editor.jumpForward",
    "tui.editor.jumpBackward",
    "tui.editor.pageUp",
    "tui.editor.pageDown",
    "tui.editor.deleteCharBackward",
    "tui.editor.deleteCharForward",
    "tui.editor.deleteWordBackward",
    "tui.editor.deleteWordForward",
    "tui.editor.deleteToLineStart",
    "tui.editor.deleteToLineEnd",
    "tui.editor.yank",
    "tui.editor.yankPop",
    "tui.editor.undo",
    "tui.input.newLine",
    "tui.input.submit",
    "tui.input.tab",
    "tui.input.copy",
    "tui.select.up",
    "tui.select.down",
    "tui.select.pageUp",
    "tui.select.pageDown",
    "tui.select.confirm",
    "tui.select.cancel",
    // app ids — pi/packages/coding-agent/src/core/keybindings.ts:66-207 @v0.83.0
    "app.interrupt",
    "app.clear",
    "app.exit",
    "app.suspend",
    "app.thinking.cycle",
    "app.model.cycleForward",
    "app.model.cycleBackward",
    "app.model.select",
    "app.tools.expand",
    "app.thinking.toggle",
    "app.session.toggleNamedFilter",
    "app.editor.external",
    "app.message.copy",
    "app.message.followUp",
    "app.message.dequeue",
    "app.clipboard.pasteImage",
    "app.session.new",
    "app.session.tree",
    "app.session.fork",
    "app.session.resume",
    "app.tree.foldOrUp",
    "app.tree.unfoldOrDown",
    "app.tree.editLabel",
    "app.tree.toggleLabelTimestamp",
    "app.session.togglePath",
    "app.session.toggleSort",
    "app.session.rename",
    "app.session.delete",
    "app.session.deleteNoninvasive",
    "app.models.save",
    "app.models.enableAll",
    "app.models.clearAll",
    "app.models.toggleProvider",
    "app.models.reorderUp",
    "app.models.reorderDown",
    "app.tree.filter.default",
    "app.tree.filter.noTools",
    "app.tree.filter.userOnly",
    "app.tree.filter.labeledOnly",
    "app.tree.filter.all",
    "app.tree.filter.cycleForward",
    "app.tree.filter.cycleBackward",
];

/// The modern id a legacy keybinding name renames to, or `None` when `key` is not legacy.
///
/// Port of `isLegacyKeybindingName` + the table lookup (`keybindings.ts:271-273`, `:297`).
#[must_use]
pub fn migrated_keybinding_name(key: &str) -> Option<&'static str> {
    KEYBINDING_NAME_MIGRATIONS
        .iter()
        .find(|(legacy, _)| *legacy == key)
        .map(|(_, modern)| *modern)
}

/// A keybindings config as an **insertion-ordered** key/value list.
///
/// `serde_json::Map` is a `BTreeMap` in this build (no `preserve_order` feature), so it cannot
/// carry the order `orderKeybindingsConfig` produces and `JSON.stringify` then writes. The pair is
/// kept as a `Vec` for exactly that reason.
pub type OrderedKeybindings = Vec<(String, Value)>;

/// Rename legacy keybinding ids and reorder the document.
///
/// Port of `migrateKeybindingsConfig` (`keybindings.ts:289-309` @v0.83.0). Returns the rewritten
/// config and whether anything changed — Pi's `{ config, migrated }`.
///
/// Three behaviours, all from upstream:
/// 1. **Rename** — a legacy id becomes its modern id (`:297-301`).
/// 2. **Drop-legacy-when-modern-present** — a legacy id whose modern twin is already in the
///    document is dropped entirely, and still counts as a migration (`:302-305`).
/// 3. **Order** — the result is reordered through `orderKeybindingsConfig` (`:308`, `:311-327`).
///
/// Iteration order of the input does not affect the output: the rename table is injective, so no
/// two inputs can collide on one target, and the result is reordered unconditionally. That is what
/// makes it safe to accept a `serde_json::Map` (alphabetical) where Pi has insertion order.
#[must_use]
pub fn migrate_keybindings_config(raw: &Map<String, Value>) -> (OrderedKeybindings, bool) {
    let mut migrated = false;
    let mut config: Vec<(String, Value)> = Vec::with_capacity(raw.len());

    for (key, value) in raw {
        let next_key = migrated_keybinding_name(key).unwrap_or(key.as_str());
        if next_key != key.as_str() {
            migrated = true;
            // keybindings.ts:302-305 — the modern twin wins and the legacy entry is discarded.
            if raw.contains_key(next_key) {
                continue;
            }
        }
        config.push((next_key.to_string(), value.clone()));
    }

    (order_keybindings_config(config), migrated)
}

/// Reorder a config: declared ids in [`KEYBINDING_IDS`] order first, then everything else sorted.
///
/// Port of `orderKeybindingsConfig` (`keybindings.ts:311-327` @v0.83.0). Pi's `extras` sort is
/// `Array.prototype.sort` over UTF-16 code units; for the ASCII ids in play that is byte order.
#[must_use]
fn order_keybindings_config(config: OrderedKeybindings) -> OrderedKeybindings {
    let declared: BTreeSet<&str> = KEYBINDING_IDS.iter().copied().collect();
    let mut ordered: Vec<(String, Value)> = Vec::with_capacity(config.len());

    for id in KEYBINDING_IDS {
        if let Some(entry) = config.iter().find(|(key, _)| key == id) {
            ordered.push(entry.clone());
        }
    }

    let mut extras: Vec<&(String, Value)> = config
        .iter()
        .filter(|(key, _)| !declared.contains(key.as_str()))
        .collect();
    extras.sort_by(|a, b| a.0.cmp(&b.0));
    ordered.extend(extras.into_iter().cloned());

    ordered
}

/// `JSON.stringify(config, null, 2)` over an insertion-ordered config.
///
/// Reproduces the exact bytes Pi writes at `migrations.ts:170` @v0.83.0. `serde_json`'s pretty
/// printer already uses a two-space indent, so a nested value only needs its continuation lines
/// shifted by one level.
#[must_use]
pub fn stringify_keybindings(config: &[(String, Value)]) -> String {
    if config.is_empty() {
        // `JSON.stringify({}, null, 2) === "{}"`.
        return "{}".to_string();
    }
    let mut out = String::from("{\n");
    for (index, (key, value)) in config.iter().enumerate() {
        out.push_str("  ");
        out.push_str(&Value::String(key.clone()).to_string());
        out.push_str(": ");
        let rendered = serde_json::to_string_pretty(value)
            .unwrap_or_else(|_| Value::Null.to_string())
            .replace('\n', "\n  ");
        out.push_str(&rendered);
        if index + 1 < config.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push('}');
    out
}

/// Migrate `<agent_dir>/keybindings.json` in place, renaming legacy ids.
///
/// Port of `migrateKeybindingsConfigFile` (`pi/packages/coding-agent/src/migrations.ts:157-172`
/// @v0.83.0), Pi's **fourth** `runMigrations` call (`:312`). Every failure mode upstream swallows
/// is swallowed here, in Pi's order:
///
/// * file absent → return (`:159`);
/// * unreadable / not JSON / not a plain object (null or array) → return (`:161-165`, `:170-172`);
/// * nothing renamed → return WITHOUT rewriting, so a clean file keeps its formatting (`:168`);
/// * otherwise write `${JSON.stringify(config, null, 2)}\n` (`:169`).
///
/// The write is a plain `fs::write`, matching Pi's `writeFileSync` — deliberately not
/// [`crate::lock::write_atomic`], which the auth migration uses because Pi passes `{mode: 0o600}`
/// there and here it does not.
pub fn migrate_keybindings_config_file(agent_dir: &Path) {
    let config_path = agent_dir.join("keybindings.json");
    // migrations.ts:159 — `if (!existsSync(configPath)) return;`
    let Ok(text) = std::fs::read_to_string(&config_path) else {
        return;
    };
    // migrations.ts:162-165 — a non-object top level (including `null` and an array) is left alone.
    let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let (config, migrated) = migrate_keybindings_config(&parsed);
    // migrations.ts:168 — `if (!migrated) return;`
    if !migrated {
        return;
    }
    let rendered = format!("{}\n", stringify_keybindings(&config));
    // migrations.ts:170-172 — the write is inside the `try`; a failure is ignored.
    let _ = std::fs::write(&config_path, rendered);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Map<String, Value> {
        match serde_json::from_str::<Value>(text).expect("valid json") {
            Value::Object(map) => map,
            other => panic!("expected an object, got {other}"),
        }
    }

    fn keys(config: &OrderedKeybindings) -> Vec<&str> {
        config.iter().map(|(k, _)| k.as_str()).collect()
    }

    #[test]
    fn migration_table_has_pis_59_entries_and_is_injective() {
        // keybindings.ts:209-269 @v0.83.0 — 21 editor + 4 input + 6 select + 28 app.
        assert_eq!(KEYBINDING_NAME_MIGRATIONS.len(), 59);
        let legacy: BTreeSet<&str> = KEYBINDING_NAME_MIGRATIONS
            .iter()
            .map(|(l, _)| *l)
            .collect();
        assert_eq!(legacy.len(), 59, "legacy names must be unique");
        let modern: BTreeSet<&str> = KEYBINDING_NAME_MIGRATIONS
            .iter()
            .map(|(_, m)| *m)
            .collect();
        assert_eq!(modern.len(), 59, "the rename must be injective");

        let by_prefix = |prefix: &str| {
            KEYBINDING_NAME_MIGRATIONS
                .iter()
                .filter(|(_, m)| m.starts_with(prefix))
                .count()
        };
        assert_eq!(by_prefix("tui.editor."), 21);
        assert_eq!(by_prefix("tui.input."), 4);
        assert_eq!(by_prefix("tui.select."), 6);
        assert_eq!(by_prefix("app."), 28);

        // Every rename target must be a DECLARED id, or the migration writes a dead entry.
        let declared: BTreeSet<&str> = KEYBINDING_IDS.iter().copied().collect();
        for (legacy, modern) in KEYBINDING_NAME_MIGRATIONS {
            assert!(
                declared.contains(modern),
                "{legacy} renames to {modern}, which is not in KEYBINDING_IDS"
            );
        }
    }

    #[test]
    fn keybinding_ids_are_pis_declaration_order() {
        assert_eq!(KEYBINDING_IDS.len(), 73, "31 tui.* + 42 app.*");
        let unique: BTreeSet<&str> = KEYBINDING_IDS.iter().copied().collect();
        assert_eq!(unique.len(), 73);
        // `KEYBINDINGS = { ...TUI_KEYBINDINGS, ...app }` — every tui id precedes every app id.
        let first_app = KEYBINDING_IDS
            .iter()
            .position(|id| id.starts_with("app."))
            .expect("app ids present");
        assert_eq!(first_app, 31);
        assert!(KEYBINDING_IDS[..31].iter().all(|id| id.starts_with("tui.")));
        assert!(KEYBINDING_IDS[31..].iter().all(|id| id.starts_with("app.")));
    }

    #[test]
    fn legacy_names_are_renamed_and_reordered_into_pis_declaration_order() {
        // The area file's CFG-048 Verify fixture.
        let raw = parse(r#"{"cursorUp":"ctrl+p","interrupt":"ctrl+q","app.clear":"ctrl+k"}"#);
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert!(migrated);
        // `tui.editor.cursorUp` is KEYBINDING_IDS[0]; `app.interrupt` precedes `app.clear`.
        assert_eq!(
            keys(&config),
            ["tui.editor.cursorUp", "app.interrupt", "app.clear"]
        );
        assert_eq!(config[0].1, Value::String("ctrl+p".into()));
        assert_eq!(config[1].1, Value::String("ctrl+q".into()));
    }

    #[test]
    fn a_legacy_name_is_dropped_when_its_modern_twin_is_present() {
        // keybindings.ts:302-305 — the modern entry wins and `migrated` is still true.
        let raw = parse(r#"{"interrupt":"ctrl+q","app.interrupt":"ctrl+e"}"#);
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert!(migrated);
        assert_eq!(keys(&config), ["app.interrupt"]);
        assert_eq!(config[0].1, Value::String("ctrl+e".into()));
    }

    #[test]
    fn a_fully_modern_document_is_not_migrated_but_unknown_ids_survive() {
        let raw = parse(r#"{"app.interrupt":"ctrl+q","zz.custom":["a","b"]}"#);
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert!(!migrated, "no legacy name present");
        // Declared ids first, undeclared "extras" appended sorted (keybindings.ts:320-325).
        assert_eq!(keys(&config), ["app.interrupt", "zz.custom"]);
    }

    #[test]
    fn extras_are_appended_in_sorted_order_after_every_declared_id() {
        let raw = parse(r#"{"zzz":"a","aaa":"b","app.exit":"c"}"#);
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert!(!migrated);
        assert_eq!(keys(&config), ["app.exit", "aaa", "zzz"]);
    }

    #[test]
    fn array_values_are_preserved_verbatim() {
        let raw = parse(r#"{"newLine":["shift+enter","ctrl+j"]}"#);
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert!(migrated);
        assert_eq!(keys(&config), ["tui.input.newLine"]);
        assert_eq!(
            config[0].1,
            Value::Array(vec![
                Value::String("shift+enter".into()),
                Value::String("ctrl+j".into())
            ])
        );
    }

    #[test]
    fn stringify_matches_json_stringify_with_two_space_indent() {
        let config: OrderedKeybindings = vec![
            ("app.interrupt".to_string(), Value::String("ctrl+q".into())),
            (
                "tui.input.newLine".to_string(),
                Value::Array(vec![
                    Value::String("shift+enter".into()),
                    Value::String("ctrl+j".into()),
                ]),
            ),
        ];
        assert_eq!(
            stringify_keybindings(&config),
            "{\n  \"app.interrupt\": \"ctrl+q\",\n  \"tui.input.newLine\": [\n    \"shift+enter\",\n    \"ctrl+j\"\n  ]\n}"
        );
        assert_eq!(stringify_keybindings(&[]), "{}");
    }

    #[test]
    fn migrate_file_rewrites_a_legacy_file_and_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keybindings.json");
        std::fs::write(
            &path,
            r#"{"cursorUp":"ctrl+p","interrupt":"ctrl+q","app.clear":"ctrl+k"}"#,
        )
        .expect("seed");

        migrate_keybindings_config_file(dir.path());
        let after = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(
            after,
            "{\n  \"tui.editor.cursorUp\": \"ctrl+p\",\n  \"app.interrupt\": \"ctrl+q\",\n  \"app.clear\": \"ctrl+k\"\n}\n"
        );

        // migrations.ts:168 — a second run finds nothing to migrate and does not rewrite.
        let before_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        migrate_keybindings_config_file(dir.path());
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), after);
        let after_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        assert_eq!(before_mtime, after_mtime, "clean file must not be rewritten");
    }

    #[test]
    fn migrate_file_leaves_absent_malformed_and_non_object_files_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Absent — migrations.ts:159.
        migrate_keybindings_config_file(dir.path());
        assert!(!dir.path().join("keybindings.json").exists());

        // Malformed JSON — migrations.ts:170-172 swallow.
        let path = dir.path().join("keybindings.json");
        for body in ["{not json", "null", "[\"cursorUp\"]", "42"] {
            std::fs::write(&path, body).expect("seed");
            migrate_keybindings_config_file(dir.path());
            assert_eq!(
                std::fs::read_to_string(&path).expect("read back"),
                body,
                "{body} must be left untouched"
            );
        }
    }

    #[test]
    fn every_legacy_name_is_migrated_in_one_pass() {
        let mut raw = Map::new();
        for (legacy, _) in KEYBINDING_NAME_MIGRATIONS {
            raw.insert(legacy.to_string(), Value::String("ctrl+a".into()));
        }
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert!(migrated);
        assert_eq!(config.len(), 59);
        for (key, _) in &config {
            assert!(
                migrated_keybinding_name(key).is_none(),
                "{key} is still a legacy name after migration"
            );
        }
    }
}
