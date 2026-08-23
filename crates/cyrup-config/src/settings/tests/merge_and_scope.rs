use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::error::ConfigError;
use crate::settings::*;

/// `shellPath` supports a leading `~` (settings-manager.ts:101), which Pi honors by running the
/// getter through `normalizePath` (`getShellPath`, settings-manager.ts:883-886) exactly as it
/// does for `sessionDir`. Regression guard for CFG-031: the raw `~/bin/bash` reached
/// `ShellConfig::resolve`, failed `Path::exists`, and broke every bash command.
#[test]
fn shell_path_is_tilde_expanded_like_session_dir() {
    let Some(home) = directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    else {
        return; // no home on this host; expansion is a no-op by contract
    };

    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "shellPath": "~/bin/bash", "sessionDir": "~/sessions" }"#,
    );
    let mgr = SettingsManager::load(store, true);
    let effective = mgr.effective();

    let shell = effective.shell_path().expect("shellPath is configured");
    assert!(
        !shell.starts_with('~'),
        "shellPath must be tilde-expanded before it reaches the shell resolver, got {shell}"
    );
    assert_eq!(shell, home.join("bin/bash").to_string_lossy());
    // Same treatment as the sibling getter Pi normalizes.
    assert_eq!(
        effective.session_dir().as_deref(),
        Some(home.join("sessions").to_string_lossy().as_ref())
    );

    // A bare `~` expands to the home dir itself, and an absolute path is untouched.
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "shellPath": "~" }"#);
    let mgr = SettingsManager::load(store, true);
    assert_eq!(
        mgr.effective().shell_path().as_deref(),
        Some(home.to_string_lossy().as_ref())
    );

    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "shellPath": "/bin/zsh" }"#);
    let mgr = SettingsManager::load(store, true);
    assert_eq!(mgr.effective().shell_path().as_deref(), Some("/bin/zsh"));
}

#[test]
fn external_editor_blank_setting_falls_through() {
    // Pi `getExternalEditorCommand` (settings-manager.ts:846-848) only honors a configured
    // editor when it is a non-empty (after-trim) string; empty/whitespace falls through.
    let env = crate::env::EnvVars {
        visual: Some("vim".to_string()),
        ..Default::default()
    };
    let default_editor = if cfg!(windows) { "notepad" } else { "nano" };

    // whitespace-only configured editor is treated as unset -> VISUAL
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "externalEditor": "   " }"#);
    let mgr = SettingsManager::load(store, true);
    assert_eq!(mgr.effective().external_editor(&env), "vim");

    // empty-string configured editor is treated as unset -> VISUAL
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "externalEditor": "" }"#);
    let mgr = SettingsManager::load(store, true);
    assert_eq!(mgr.effective().external_editor(&env), "vim");

    // empty configured editor with no VISUAL/EDITOR -> platform default
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "externalEditor": "  " }"#);
    let mgr = SettingsManager::load(store, true);
    assert_eq!(
        mgr.effective()
            .external_editor(&crate::env::EnvVars::default()),
        default_editor
    );

    // a non-blank configured editor wins (returned verbatim, including surrounding content)
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "externalEditor": "code --wait" }"#,
    );
    let mgr = SettingsManager::load(store, true);
    assert_eq!(mgr.effective().external_editor(&env), "code --wait");
}

#[test]
fn deep_merge_precedence_and_nested() {
    // A-07-1: nested objects merge per-key; arrays replace; CLI > project > global.
    let global = serde_json::json!({
        "defaultModel": "g-model",
        "retry": { "enabled": true, "maxRetries": 3 },
        "list": [1, 2, 3]
    });
    let project = serde_json::json!({
        "defaultModel": "p-model",
        "retry": { "maxRetries": 5 },
        "list": [9]
    });
    let cli = serde_json::json!({ "defaultModel": "c-model" });

    let merged = deep_merge(&global, &project);
    let merged = deep_merge(&merged, &cli);

    assert_eq!(merged["defaultModel"], "c-model");
    // nested per-key merge: enabled kept from global, maxRetries overridden by project
    assert_eq!(merged["retry"]["enabled"], true);
    assert_eq!(merged["retry"]["maxRetries"], 5);
    // arrays replace wholesale
    assert_eq!(merged["list"], serde_json::json!([9]));
}

#[test]
fn unknown_keys_survive_roundtrip() {
    // A-07-8 / R-07-004
    let text = r#"{ "defaultModel": "x", "someFutureKey": { "a": 1 }, "topUnknown": 7 }"#;
    let s = Settings::parse(text).unwrap();
    let out = s.to_pretty();
    let reparsed = Settings::parse(&out).unwrap();
    assert_eq!(
        reparsed.get("someFutureKey"),
        Some(&serde_json::json!({"a": 1}))
    );
    assert_eq!(reparsed.get("topUnknown"), Some(&serde_json::json!(7)));
    assert_eq!(reparsed.get("defaultModel"), Some(&serde_json::json!("x")));
}

/// CFG-059 — the precedence MODEL is pi's: exactly two persistent layers, `global ◁ project`,
/// and the only tier above project is the TRANSIENT `applyOverrides`.
///
/// Presence before absence: the override is first shown to WIN over project (so this is a
/// statement about precedence, not a dead call), and only then shown not to survive a
/// recompute. The old shape of this test asserted a third `cli` layer that outranked project
/// AND persisted; pi has no such tier — `applyOverrides` has exactly two v0.83.0 call sites
/// (`examples/sdk/10-settings.ts:17`, `test/test-harness.ts:395`) and zero production callers.
#[test]
fn project_outranks_global_and_the_only_tier_above_project_is_transient() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "defaultModel": "g", "theme": "light" }"#,
    );
    store.seed(SettingsScope::Project, r#"{ "defaultModel": "p" }"#);

    // Two layers: project wins for its own key, global still supplies the rest.
    let mut mgr = SettingsManager::load(store.clone(), true);
    assert_eq!(mgr.effective().default_model(), Some("p".to_string()));
    assert_eq!(mgr.effective().theme(), Some("light".to_string()));

    // PRESENCE: an override outranks the project layer while it is applied.
    let mut overrides = Settings::new();
    overrides.set_field("defaultModel", "c").unwrap();
    mgr.apply_overrides(&overrides);
    assert_eq!(mgr.effective().default_model(), Some("c".to_string()));
    assert_eq!(mgr.effective().theme(), Some("light".to_string()));

    // ABSENCE: it is not a layer — every recompute path drops it and project wins again.
    mgr.reload().unwrap();
    assert_eq!(mgr.effective().default_model(), Some("p".to_string()));
    mgr.apply_overrides(&overrides);
    mgr.set_project_trusted(false);
    assert_eq!(mgr.effective().default_model(), Some("g".to_string()));
    mgr.set_project_trusted(true);
    assert_eq!(mgr.effective().default_model(), Some("p".to_string()));
}

/// CFG-059 × CFG-057 — an override cannot supply a global-only key. Upstream expresses
/// global-only-ness at the getter (`getGlobalSettings()` returns `this.globalSettings`,
/// settings-manager.ts:442-444) and `applyOverrides` only ever touches `this.settings`, so
/// upstream's override path cannot reach `httpProxy` / `defaultProjectTrust` either. cyrup
/// implements the same guarantee at the merge, so the strip has to cover this path too.
#[test]
fn an_override_cannot_supply_a_global_only_key() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "httpProxy": "http://global:8080", "defaultModel": "g" }"#,
    );
    let mut mgr = SettingsManager::load(store, false);

    let overrides = Settings::parse(
        r#"{ "httpProxy": "http://override:9", "defaultProjectTrust": "always", "defaultModel": "o" }"#,
    )
    .unwrap();
    mgr.apply_overrides(&overrides);

    // PRESENCE: a non-global-only key from the same override document DID land.
    assert_eq!(mgr.effective().default_model(), Some("o".to_string()));
    // ABSENCE: the two global-only keys did not.
    assert_eq!(
        mgr.effective().http_proxy(),
        Some("http://global:8080".to_string())
    );
    assert_eq!(
        mgr.effective().default_project_trust(),
        DefaultProjectTrust::Ask
    );
}

#[test]
fn per_layer_resource_path_accessors_read_a_single_scope() {
    // gap-09 #26 cross-layer wiring: `global()`/`project()` expose the per-layer split so a
    // consumer (session-svc DiscoveryConfig) can gate global- vs project-scope resource
    // overrides independently — NOT from the merged `effective()` view (which would let a
    // project list silently widen the global scope, or vice-versa).
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "skills": ["g-skill"], "prompts": ["g-prompt"], "themes": ["g-theme"] }"#,
    );
    store.seed(
        SettingsScope::Project,
        r#"{ "skills": ["p-skill-a", "p-skill-b"], "prompts": ["p-prompt"] }"#,
    );
    let mgr = SettingsManager::load(store, true);

    // Each layer reports ONLY its own list (no merge).
    assert_eq!(mgr.global().skill_paths(), vec!["g-skill".to_string()]);
    assert_eq!(
        mgr.project().skill_paths(),
        vec!["p-skill-a".to_string(), "p-skill-b".to_string()]
    );
    assert_eq!(
        mgr.global().prompt_template_paths(),
        vec!["g-prompt".to_string()]
    );
    assert_eq!(
        mgr.project().prompt_template_paths(),
        vec!["p-prompt".to_string()]
    );
    // `themes` set only globally: project layer is empty (NOT inheriting the global value).
    assert_eq!(mgr.global().theme_paths(), vec!["g-theme".to_string()]);
    assert!(mgr.project().theme_paths().is_empty());
    // The merged effective view still unions them (sanity: per-layer != effective).
    assert!(mgr.effective().skill_paths().len() >= mgr.global().skill_paths().len());
}

#[test]
fn project_not_loaded_until_trusted() {
    // R-07-002
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "defaultModel": "g" }"#);
    store.seed(SettingsScope::Project, r#"{ "defaultModel": "p" }"#);

    let mut mgr = SettingsManager::load(store, false);
    assert_eq!(mgr.effective().default_model(), Some("g".to_string()));
    mgr.set_project_trusted(true);
    assert_eq!(mgr.effective().default_model(), Some("p".to_string()));
    mgr.set_project_trusted(false);
    assert_eq!(mgr.effective().default_model(), Some("g".to_string()));
}

#[test]
fn default_project_trust_is_global_only() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "defaultProjectTrust": "always" }"#,
    );
    // project tries to set it but it must be stripped
    store.seed(
        SettingsScope::Project,
        r#"{ "defaultProjectTrust": "never" }"#,
    );
    let mgr = SettingsManager::load(store, true);
    assert_eq!(
        mgr.effective().default_project_trust(),
        DefaultProjectTrust::Always
    );
}

/// CFG-057 — RED before the fix. Pi reads `httpProxy` off the raw GLOBAL document
/// (`main.ts:537` / `:801`, both `getGlobalSettings().httpProxy`) and documents it as
/// "Global setting only." (`packages/coding-agent/docs/settings.md:87` @v0.83.0), so a
/// project `.cyrup/settings.json` must not be able to rewrite the session's egress — not even
/// a TRUSTED one, since approving a project is not approving a proxy.
#[test]
fn http_proxy_is_global_only() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "httpProxy": "http://global:8080" }"#,
    );
    store.seed(
        SettingsScope::Project,
        r#"{ "httpProxy": "http://project:9090" }"#,
    );
    let mgr = SettingsManager::load(store, true);
    assert_eq!(
        mgr.effective().http_proxy(),
        Some("http://global:8080".to_string())
    );

    // And with no global value the project one supplies nothing at all.
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Project,
        r#"{ "httpProxy": "http://project:9090" }"#,
    );
    let mgr = SettingsManager::load(store, true);
    assert_eq!(mgr.effective().http_proxy(), None);
}

/// CFG-060 — **COVERAGE, not a red-before proof, and the distinction is the point.** The fix is
/// the REMOVAL of the accessor's `&EnvVars` parameter, so this test cannot be written against
/// the pre-fix API at all: it would not compile. What the pre-fix code did, stated so the
/// change is auditable — `http_proxy(&EnvVars { http_proxy: Some("http://ambient:3128"), .. })`
/// with NO `httpProxy` key in either document returned `Some("http://ambient:3128")`, because
/// the body ended in `.or_else(|| env.http_proxy.clone())`.
///
/// Why that was wrong rather than merely redundant. pi calls
/// `applyHttpProxySettings(getGlobalSettings().httpProxy)` (`main.ts:537`, `:801` @v0.83.0),
/// which is `process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`
/// (`http-dispatcher.ts:43-48`) — the two names are filled INDEPENDENTLY. With an ambient
/// `HTTP_PROXY=http://ambient:3128` and `"httpProxy": "http://setting:8080"`, upstream leaves
/// `HTTP_PROXY` ambient and sets `HTTPS_PROXY` to the SETTING, so an https target proxies
/// through `http://setting:8080`. Feeding the ambient value back through this accessor into
/// `configure_http_proxy` would have made `http://ambient:3128` the configured proxy for both
/// names and lost the setting for https targets entirely. The ambient-wins half of `??=` is
/// already ported, once, in `node_http_proxy::get_proxy_env`.
#[test]
fn http_proxy_is_the_setting_alone_and_takes_no_environment() {
    // Unset on both layers: the accessor has nothing to fall back TO any more.
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "defaultModel": "m" }"#);
    let mgr = SettingsManager::load(store, true);
    assert_eq!(
        mgr.effective().http_proxy(),
        None,
        "no httpProxy key means no configured proxy, whatever the ambient environment holds"
    );

    // Set: trimmed, and an all-whitespace value is `!proxy` upstream (http-dispatcher.ts:44-45).
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "httpProxy": "  http://setting:8080  " }"#,
    );
    let mgr = SettingsManager::load(store, true);
    assert_eq!(
        mgr.effective().http_proxy(),
        Some("http://setting:8080".to_string())
    );

    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "httpProxy": "   " }"#);
    let mgr = SettingsManager::load(store, true);
    assert_eq!(mgr.effective().http_proxy(), None);
}

/// CFG-061 — **RED before the fix**: `EffectiveSettings::packages()` was
/// `from_value::<Vec<PackageSource>>(v.clone()).ok().unwrap_or_default()`, so the `Err` from
/// entry 4 collapsed the whole array and this asserted 9 against 0. pi's `getPackages`
/// (`settings-manager.ts:969-971` @v0.83.0) is `[...(this.settings.packages ?? [])]` — a
/// verbatim copy with no parsing at all, so a malformed entry travels downstream and is
/// rejected on its own.
#[test]
fn one_malformed_package_entry_does_not_discard_the_other_nine() {
    let s = EffectiveSettings::from_settings(
        Settings::parse(
            r#"{"packages": [
                 "a", "b", "c",
                 42,
                 "e", "f", "g", "h", "i", "j"
               ]}"#,
        )
        .unwrap(),
    );
    let (pkgs, errors) = s.packages_with_errors();
    assert_eq!(
        pkgs.len(),
        9,
        "nine well-formed entries survive the tenth being a number"
    );
    assert_eq!(s.packages().len(), 9, "the error-free accessor agrees");
    assert_eq!(errors.len(), 1, "and the bad entry is reported, not silent");
    assert!(
        errors
            .first()
            .is_some_and(|e| e.starts_with("settings `packages[3]`")),
        "the diagnostic names the index: {errors:?}"
    );
    assert_eq!(
        pkgs.first(),
        Some(&PackageSource::Name("a".to_string())),
        "and the entries before the bad one are kept, not just the ones after"
    );
}

/// CFG-062 — **RED before the fix** on both halves: the written document contained
/// `"shellPath": null` / `"terminal": {"showImages": null}` and both `contains` assertions
/// failed. pi's clearing setters assign `undefined` (`setShellPath`
/// settings-manager.ts:883-887, `setShellCommandPrefix` `:914-918`, `setNpmCommand`
/// `:924-928` @v0.83.0) and `persistScopedSettings` writes through
/// `JSON.stringify(mergedSettings, null, 2)` (`:605`), which omits undefined-valued properties
/// at every depth — so upstream cannot produce a `null` in a settings document at all.
#[tokio::test]
async fn clearing_a_key_removes_it_rather_than_writing_json_null() {
    let store = Arc::new(InMemorySettingsStore::new());
    let mut mgr = SettingsManager::load(store.clone(), true);

    mgr.set(SettingsScope::Global, "shellPath", Some("~/bin/bash"))
        .await
        .unwrap();
    mgr.set_nested(
        SettingsScope::Global,
        &["terminal", "showImages"],
        Value::Bool(true),
    )
    .await
    .unwrap();
    let written = store.read(SettingsScope::Global).unwrap().unwrap();
    assert!(written.contains("shellPath"), "precondition: {written}");
    assert!(written.contains("showImages"), "precondition: {written}");

    // Clear both. `None::<&str>` serializes to `Value::Null`, which is the only way a Rust
    // caller can express pi's `undefined`.
    mgr.set(SettingsScope::Global, "shellPath", None::<&str>)
        .await
        .unwrap();
    mgr.set_nested(
        SettingsScope::Global,
        &["terminal", "showImages"],
        Value::Null,
    )
    .await
    .unwrap();

    let written = store.read(SettingsScope::Global).unwrap().unwrap();
    assert!(
        !written.contains("shellPath"),
        "the key must be GONE, not present-and-null: {written}"
    );
    assert!(
        !written.contains("showImages"),
        "nested leaves clear the same way: {written}"
    );
    assert!(
        !written.contains("null"),
        "no null survives anywhere in the document: {written}"
    );
    assert!(
        written.contains("terminal"),
        "clearing a leaf must not delete its parent object: {written}"
    );
    assert_eq!(mgr.effective().shell_path(), None);
}

/// CFG-062, the merge half — **recorded as a REFUTATION, and it is not a bug.** The item's
/// Impact claims cyrup's `deep_merge` lacks pi's undefined-skip and that "a project
/// `npmCommand: null` blanks the global value where pi has no way to express that state at
/// all". Both clauses are false. `serde_json` has no `undefined`, so a key absent from the
/// project map is structurally skipped — the skip pi spells at `settings-manager.ts:139-141`
/// @v0.83.0 (and at `:149-152` of the v0.84.1 `deepMergeObjects`) is unrepresentable here. And
/// a hand-written `"npmCommand": null` in a project file IS expressible upstream: JSON.parse
/// yields `null`, `overrideValue === undefined` is false, so pi's merge takes the null too and
/// `getNpmCommand`'s `this.settings.npmCommand ? … : undefined` then reads it as unset —
/// exactly what cyrup does. The write path was the only divergence.
#[test]
fn a_project_null_blanks_a_global_value_on_both_sides() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "npmCommand": ["pnpm"], "defaultModel": "m" }"#,
    );
    store.seed(SettingsScope::Project, r#"{ "npmCommand": null }"#);
    let mgr = SettingsManager::load(store, true);
    assert_eq!(
        mgr.effective().npm_command(),
        None,
        "pi's deepMergeSettings skips undefined, not null — the null wins there too"
    );
    assert_eq!(
        mgr.effective().default_model(),
        Some("m".to_string()),
        "and it is scoped to the one key, not the document"
    );
}

#[tokio::test]
async fn set_field_preserves_unknown_keys() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "futureKey": 42, "defaultModel": "old" }"#,
    );
    let mut mgr = SettingsManager::load(store.clone(), false);
    mgr.set(SettingsScope::Global, "defaultModel", "new")
        .await
        .unwrap();
    let raw = store.read(SettingsScope::Global).unwrap().unwrap();
    let s = Settings::parse(&raw).unwrap();
    assert_eq!(s.get("futureKey"), Some(&serde_json::json!(42)));
    assert_eq!(s.get("defaultModel"), Some(&serde_json::json!("new")));
}

#[tokio::test]
async fn project_write_requires_trust() {
    let store = Arc::new(InMemorySettingsStore::new());
    let mut mgr = SettingsManager::load(store, false);
    let err = mgr.set(SettingsScope::Project, "defaultModel", "x").await;
    assert!(matches!(err, Err(ConfigError::Untrusted)));
}
