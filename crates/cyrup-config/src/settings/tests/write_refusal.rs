use std::sync::Arc;

use crate::error::ConfigError;
use crate::settings::*;
#[test]
fn apply_overrides_deep_merges_onto_effective() {
    // settings-manager.ts:503-505 — runtime overrides deep-merge onto the effective view.
    let store = Arc::new(InMemorySettingsStore::new());
    let mut mgr = SettingsManager::load(store.clone(), false);
    assert!(mgr.effective().compaction_enabled());
    assert_eq!(mgr.effective().compaction_reserve_tokens(), 16384);

    let overrides =
        Settings::parse(r#"{ "compaction": { "reserveTokens": 4096 }, "quietStartup": true }"#)
            .unwrap();
    mgr.apply_overrides(&overrides);
    // nested merge preserves the sibling `enabled` default while overriding reserveTokens.
    assert!(mgr.effective().compaction_enabled());
    assert_eq!(mgr.effective().compaction_reserve_tokens(), 4096);
    assert!(mgr.effective().quiet_startup());

    // transient: a reload recomputes from the layers and drops the overrides.
    mgr.reload().unwrap();
    assert_eq!(mgr.effective().compaction_reserve_tokens(), 16384);
    assert!(!mgr.effective().quiet_startup());
}

#[tokio::test]
async fn enable_analytics_generates_tracking_id() {
    // settings-manager.ts:943-951
    let store = Arc::new(InMemorySettingsStore::new());
    let mut mgr = SettingsManager::load(store.clone(), false);
    assert!(mgr.effective().tracking_id().is_none());
    mgr.set_enable_analytics(true).await.unwrap();
    assert!(mgr.effective().enable_analytics());
    let id = mgr.effective().tracking_id().unwrap();
    assert_eq!(id.len(), 36); // canonical UUID form
    // opting out doesn't regenerate / clear the id; opting back in keeps the same id
    mgr.set_enable_analytics(false).await.unwrap();
    mgr.set_enable_analytics(true).await.unwrap();
    assert_eq!(mgr.effective().tracking_id().unwrap(), id);
}

// -----------------------------------------------------------------------
// CFG-001 — a writer must REFUSE a scope whose file it could not parse, never rewrite it.
//
// Pi guards every writer: `save()` (settings-manager.ts ≈:614-628) opens with
// `if (this.globalSettingsLoadError) { return; }` and `saveProjectSettings()` (≈:633-646) has
// the mirror. Before this fix cyrup's `set`/`set_nested`/`persist_nested` all did
// `match current.map(Settings::parse) { Some(Ok(s)) => s, _ => Settings::default() }`, so a
// trailing comma in `~/.cyrup/settings.json` meant the next `/config` toggle rewrote the whole
// file as `{"<key>": <value>}` — every other setting gone.
//
// The assertions are BYTE-level (`assert_eq!(after, MALFORMED)`), not "the key I wrote is
// absent": the whole point is that the user's file is left exactly as they left it.
// -----------------------------------------------------------------------

/// A realistic corruption: a trailing comma before `}`, plus settings worth losing.
const MALFORMED: &str = "{\n  \"defaultModel\": \"anthropic/claude-opus-4\",\n  \"theme\": \"dark\",\n  \"editorPaddingX\": 2,\n}\n";

fn malformed_global() -> (Arc<InMemorySettingsStore>, SettingsManager) {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, MALFORMED);
    let mgr = SettingsManager::load(store.clone(), false);
    (store, mgr)
}

fn assert_refused(result: Result<(), ConfigError>, expected_scope: SettingsScope) {
    let described = format!("{result:?}");
    // The refusal must name the scope it protected AND carry the underlying cause, so a
    // `/config` toggle can tell the user which file to go fix.
    let matched = matches!(
        &result,
        Err(ConfigError::SettingsWriteRefused { scope, message })
            if *scope == expected_scope && message.contains("parse error")
    );
    assert!(
        matched,
        "expected SettingsWriteRefused{{{expected_scope:?}, ..parse error..}}, got {described}"
    );
}

#[tokio::test]
async fn cfg001_set_refuses_to_clobber_a_malformed_file() {
    let (store, mut mgr) = malformed_global();
    // The load recorded the failure (R-00-009) and latched the scope (Pi globalSettingsLoadError).
    assert!(
        mgr.load_error(SettingsScope::Global).is_some(),
        "the scope is latched"
    );

    assert_refused(
        mgr.set(SettingsScope::Global, "theme", "light").await,
        SettingsScope::Global,
    );

    let after = store.read(SettingsScope::Global).unwrap().unwrap();
    assert_eq!(
        after, MALFORMED,
        "the malformed file is byte-for-byte unchanged"
    );
}

#[tokio::test]
async fn cfg001_set_nested_refuses_to_clobber_a_malformed_file() {
    let (store, mut mgr) = malformed_global();

    assert_refused(
        mgr.set_nested(
            SettingsScope::Global,
            &["terminal", "showImages"],
            false.into(),
        )
        .await,
        SettingsScope::Global,
    );

    let after = store.read(SettingsScope::Global).unwrap().unwrap();
    assert_eq!(
        after, MALFORMED,
        "the malformed file is byte-for-byte unchanged"
    );
}

#[tokio::test]
async fn cfg001_persist_nested_refuses_to_clobber_a_malformed_file() {
    let (store, mgr) = malformed_global();

    assert_refused(
        mgr.persist_nested(SettingsScope::Global, &["outputPad"], 0.into())
            .await,
        SettingsScope::Global,
    );

    let after = store.read(SettingsScope::Global).unwrap().unwrap();
    assert_eq!(
        after, MALFORMED,
        "the malformed file is byte-for-byte unchanged"
    );
}

#[tokio::test]
async fn cfg001_convenience_setters_refuse_too() {
    // Every `/config`-reachable convenience setter routes through one of the three writers, so
    // each inherits the guard — including `set_enable_analytics`, which owns its own `with_lock`.
    let (store, mut mgr) = malformed_global();

    assert_refused(mgr.set_editor_padding_x(3.0).await, SettingsScope::Global);
    assert_refused(mgr.set_show_images(false).await, SettingsScope::Global);
    assert_refused(mgr.set_image_width_cells(40.0).await, SettingsScope::Global);
    assert_refused(
        mgr.set_autocomplete_max_visible(9.0).await,
        SettingsScope::Global,
    );
    assert_refused(
        mgr.set_http_idle_timeout_ms(1000.0).await,
        SettingsScope::Global,
    );
    assert_refused(mgr.set_enable_analytics(true).await, SettingsScope::Global);

    let after = store.read(SettingsScope::Global).unwrap().unwrap();
    assert_eq!(
        after, MALFORMED,
        "six refused writes later, still untouched"
    );
}

#[tokio::test]
async fn cfg001_project_scope_is_latched_independently() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "theme": "dark" }"#);
    store.seed(SettingsScope::Project, MALFORMED);
    let mut mgr = SettingsManager::load(store.clone(), true);

    assert!(mgr.load_error(SettingsScope::Project).is_some());
    assert!(
        mgr.load_error(SettingsScope::Global).is_none(),
        "a healthy scope is not latched"
    );

    assert_refused(
        mgr.set(SettingsScope::Project, "quietStartup", true).await,
        SettingsScope::Project,
    );
    assert_eq!(
        store.read(SettingsScope::Project).unwrap().unwrap(),
        MALFORMED
    );

    // The healthy GLOBAL scope still writes — the guard is per-scope, not a global kill switch.
    mgr.set(SettingsScope::Global, "quietStartup", true)
        .await
        .unwrap();
    assert!(mgr.effective().quiet_startup());
}

#[tokio::test]
async fn cfg001_corruption_between_load_and_write_is_also_refused() {
    // The second half of the fix: the file loaded FINE (no latch), then something corrupted it
    // before the locked read-modify-write. The in-closure `Some(Err(_))` arm must abandon the
    // write and surface the refusal rather than starting from an empty document.
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "theme": "dark" }"#);
    let mut mgr = SettingsManager::load(store.clone(), false);
    assert!(
        mgr.load_error(SettingsScope::Global).is_none(),
        "loaded clean"
    );

    store.seed(SettingsScope::Global, MALFORMED); // corrupted behind our back

    assert_refused(
        mgr.set(SettingsScope::Global, "theme", "light").await,
        SettingsScope::Global,
    );
    assert_eq!(
        store.read(SettingsScope::Global).unwrap().unwrap(),
        MALFORMED
    );

    assert_refused(
        mgr.set_nested(
            SettingsScope::Global,
            &["terminal", "showImages"],
            true.into(),
        )
        .await,
        SettingsScope::Global,
    );
    assert_refused(
        mgr.persist_nested(SettingsScope::Global, &["outputPad"], 1.into())
            .await,
        SettingsScope::Global,
    );
    assert_refused(mgr.set_enable_analytics(true).await, SettingsScope::Global);
    assert_eq!(
        store.read(SettingsScope::Global).unwrap().unwrap(),
        MALFORMED
    );
}

#[tokio::test]
async fn cfg001_repairing_the_file_and_reloading_restores_writability() {
    let (store, mut mgr) = malformed_global();
    assert!(
        mgr.set(SettingsScope::Global, "theme", "light")
            .await
            .is_err()
    );

    // The user fixes the trailing comma and cyrup reloads: the latch clears and writes resume.
    store.seed(
        SettingsScope::Global,
        r#"{ "defaultModel": "anthropic/claude-opus-4" }"#,
    );
    mgr.reload().unwrap();
    assert!(
        mgr.load_error(SettingsScope::Global).is_none(),
        "latch cleared on a clean reload"
    );

    mgr.set(SettingsScope::Global, "theme", "light")
        .await
        .unwrap();
    let after = Settings::parse(&store.read(SettingsScope::Global).unwrap().unwrap()).unwrap();
    assert_eq!(after.get("theme"), Some(&serde_json::json!("light")));
    assert_eq!(
        after.get("defaultModel"),
        Some(&serde_json::json!("anthropic/claude-opus-4")),
        "and the repaired file's other keys survive"
    );
}

#[tokio::test]
async fn cfg001_an_absent_file_is_still_created() {
    // The refusal must not break first-run: `None` (no file) is not a parse failure.
    let store = Arc::new(InMemorySettingsStore::new());
    let mut mgr = SettingsManager::load(store.clone(), false);
    assert!(mgr.load_error(SettingsScope::Global).is_none());

    mgr.set(SettingsScope::Global, "theme", "light")
        .await
        .unwrap();
    mgr.set_nested(
        SettingsScope::Global,
        &["terminal", "showImages"],
        true.into(),
    )
    .await
    .unwrap();
    let after = Settings::parse(&store.read(SettingsScope::Global).unwrap().unwrap()).unwrap();
    assert_eq!(after.get("theme"), Some(&serde_json::json!("light")));
}

/// CFG-003: the PER-LAYER `packages()` accessor (Pi reads `projectSettings.packages` and
/// `globalSettings.packages` separately, package-manager.ts:891-898) parses entry-by-entry, so
/// one malformed entry costs only that entry — not the array, and not the settings document.
#[test]
fn per_layer_packages_reports_a_bad_entry_and_keeps_the_good_ones() {
    let s = Settings::parse(
        r#"{"defaultModel":"anthropic/x","packages":[17,"good-pkg",{"source":"filtered","skills":["a"]}]}"#,
    )
    .unwrap();
    let (pkgs, errors) = s.packages_with_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("packages[0]"), "{errors:?}");
    assert_eq!(pkgs.len(), 2, "the two well-formed entries survive");
    assert_eq!(pkgs[0].source(), "good-pkg");
    assert_eq!(pkgs[1].source(), "filtered");
    assert_eq!(pkgs[1].filters().1, Some(&["a".to_string()][..]));
    // The rest of the document is untouched.
    assert_eq!(
        EffectiveSettings::from_settings(s)
            .default_model()
            .as_deref(),
        Some("anthropic/x")
    );
}

/// A non-array `packages` is itself reported rather than silently treated as absent.
#[test]
fn per_layer_packages_reports_a_non_array_value() {
    let s = Settings::parse(r#"{"packages":"oops"}"#).unwrap();
    let (pkgs, errors) = s.packages_with_errors();
    assert!(pkgs.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("must be an array"), "{errors:?}");
}

/// CFG-004: the per-layer `extension_paths()` accessor exists (the merged view cannot say which
/// scope declared an entry, and project entries are trust-gated independently).
#[test]
fn per_layer_extension_paths() {
    let s = Settings::parse(r#"{"extensions":["a","!b/*"]}"#).unwrap();
    assert_eq!(
        s.extension_paths(),
        vec!["a".to_string(), "!b/*".to_string()]
    );
    assert!(Settings::default().extension_paths().is_empty());
}
