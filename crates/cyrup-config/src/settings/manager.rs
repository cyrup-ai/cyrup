//! The two-layer settings facade (arch-07 §3.3): `global ◁ project` with a memoized merge, the
//! transient override path, and the writers that refuse a scope they could not parse (CFG-001).

use std::sync::Arc;

use serde_json::{Map, Value};

use super::effective::EffectiveSettings;
use super::layer::{Settings, strip_global_only};
use super::merge::deep_merge;
use super::store::SettingsStore;
use super::types::{MermaidRenderingMode, SettingsScope};
use crate::error::{ConfigError, ScopedError};

/// The layered settings facade (arch-07 §3.3). Holds the two layers + a memoized merge.
///
/// **Exactly two persistent layers, `global ◁ project`, matching pi's
/// `this.settings = deepMergeSettings(this.globalSettings, this.projectSettings)`
/// (settings-manager.ts:305, and again at `:466`, `:503` @v0.83.0).** There is no CLI tier
/// upstream: the ONLY way to push a value above the project layer is the transient
/// [`Self::apply_overrides`] (`applyOverrides`, settings-manager.ts:508-510), which merges onto the
/// already-merged view and is discarded by the next recompute. CFG-059 removed a third persistent
/// `cli` layer that sat above project, was stripped by `strip_global_only`, and survived every
/// `reload()` / `set_project_trusted()` — three properties upstream's override path does not have.
pub struct SettingsManager {
    store: Arc<dyn SettingsStore>,
    global: Settings,
    project: Settings,
    effective: EffectiveSettings,
    project_trusted: bool,
    load_errors: Vec<ScopedError>,
    /// The LAST load's failure for the global scope, if any (Pi `globalSettingsLoadError`,
    /// settings-manager.ts:289). Distinct from `load_errors`, which accumulates across reloads and
    /// is drained once for display: this is the live per-scope latch every writer consults so a
    /// document cyrup could not read is never rewritten from the degraded in-memory view (CFG-001).
    global_load_error: Option<String>,
    /// The LAST load's failure for the project scope (Pi `projectSettingsLoadError`,
    /// settings-manager.ts:290). Always `None` while the project is untrusted — that scope is not
    /// read at all, and its writes are already refused with [`ConfigError::Untrusted`].
    project_load_error: Option<String>,
}

impl SettingsManager {
    /// Load global unconditionally; load project ONLY if `project_trusted` (R-07-002). A parse
    /// error degrades that scope to empty and records a `ScopedError` (R-00-009) plus the
    /// per-scope write latch (CFG-001).
    pub fn load(store: Arc<dyn SettingsStore>, project_trusted: bool) -> Self {
        let mut mgr = Self {
            store,
            global: Settings::default(),
            project: Settings::default(),
            effective: EffectiveSettings::default(),
            project_trusted,
            load_errors: Vec::new(),
            global_load_error: None,
            project_load_error: None,
        };
        mgr.reload_internal();
        mgr
    }

    /// Record this load's failure both in the drainable log and in the per-scope write latch.
    ///
    /// Pi latches on ANY failure of `loadFromStorage`, not only a JSON syntax error:
    /// `tryLoadFromStorage` (settings-manager.ts:373-383) wraps the whole load in one try/catch and
    /// hands the caught error straight to `globalSettingsLoadError`/`projectSettingsLoadError`. So a
    /// store READ failure latches here too — an unreadable file is exactly as unsafe to overwrite
    /// as an unparseable one.
    fn record_load_error(&mut self, scope: SettingsScope, message: String) {
        match scope {
            SettingsScope::Global => self.global_load_error = Some(message.clone()),
            SettingsScope::Project => self.project_load_error = Some(message.clone()),
        }
        self.load_errors.push(ScopedError { scope, message });
    }

    fn load_scope(&mut self, scope: SettingsScope) -> Settings {
        match self.store.read(scope) {
            Ok(Some(text)) => match Settings::parse(&text) {
                Ok(s) => s,
                Err(e) => {
                    self.record_load_error(scope, format!("parse error: {e}"));
                    Settings::default()
                }
            },
            Ok(None) => Settings::default(),
            Err(e) => {
                self.record_load_error(scope, e.to_string());
                Settings::default()
            }
        }
    }

    fn reload_internal(&mut self) {
        // Clear the latches first: they describe the load that is about to happen, so a user who
        // repairs the file and reloads regains the ability to write (Pi sets both fields from the
        // fresh `tryLoadFromStorage` result on every reload, settings-manager.ts:477/489-491/503-505).
        self.global_load_error = None;
        self.project_load_error = None;
        self.global = self.load_scope(SettingsScope::Global);
        self.project = if self.project_trusted {
            self.load_scope(SettingsScope::Project)
        } else {
            Settings::default()
        };
        self.recompute();
    }

    fn recompute(&mut self) {
        // Strip global-only keys from project before merge.
        let mut project = self.project.clone();
        strip_global_only(&mut project);

        let merged = deep_merge(&self.global.to_value(), &project.to_value());
        let merged = match merged {
            Value::Object(obj) => Settings::from_map(obj),
            _ => Settings::default(),
        };
        self.effective = EffectiveSettings::from_settings(merged);
    }

    pub fn effective(&self) -> &EffectiveSettings {
        &self.effective
    }

    pub fn global(&self) -> &Settings {
        &self.global
    }

    pub fn project(&self) -> &Settings {
        &self.project
    }

    pub fn project_trusted(&self) -> bool {
        self.project_trusted
    }

    /// Re-read both scopes from disk and rebuild the effective view (R-07-005/029).
    pub fn reload(&mut self) -> Result<(), ConfigError> {
        self.reload_internal();
        Ok(())
    }

    /// `applyOverrides` (Pi settings-manager.ts:508-510): deep-merge additional overrides on top of
    /// the current effective settings at runtime. These overrides are NOT persisted and are
    /// transient — any subsequent `reload`/`set_project_trusted` recomputes the merge from the
    /// global/project layers and discards them (matching Pi, where `applyOverrides` mutates the
    /// in-memory `this.settings` that `reload()` later rebuilds at `:503`).
    ///
    /// This is the ONLY override path above the project layer, and the seam an embedder or a test
    /// harness uses — upstream's own callers are `examples/sdk/10-settings.ts:17`,
    /// `test/test-harness.ts:395` (`applyOverrides(options.settings)`) and
    /// `test/utilities.ts:258` (`options.settingsOverrides`); there are ZERO production callers at
    /// v0.83.0, which is why CFG-059 deleted cyrup's persistent `cli` tier rather than keeping it.
    ///
    /// Global-only keys are stripped from the overrides. Upstream expresses global-only-ness at the
    /// GETTER — `getGlobalSettings()` returns `this.globalSettings`, never `this.settings`
    /// (settings-manager.ts:442-444) — so `applyOverrides`, which only ever touches `this.settings`,
    /// cannot influence `defaultProjectTrust` or `httpProxy` upstream either. cyrup implements the
    /// same guarantee at the merge, so the strip has to happen on this path too or CFG-057's fix
    /// would be reachable through an override.
    pub fn apply_overrides(&mut self, overrides: &Settings) {
        let mut overrides = overrides.clone();
        strip_global_only(&mut overrides);
        let merged = deep_merge(&self.effective.raw().to_value(), &overrides.to_value());
        let merged = match merged {
            Value::Object(obj) => Settings::from_map(obj),
            _ => Settings::default(),
        };
        self.effective = EffectiveSettings::from_settings(merged);
    }

    /// Trust transition: false→true loads the project scope; true→false drops it (R-07-002/012).
    pub fn set_project_trusted(&mut self, trusted: bool) {
        if self.project_trusted != trusted {
            self.project_trusted = trusted;
            self.reload_internal();
        }
    }

    /// The load-error latch for `scope`, if the last load of that scope failed (CFG-001).
    ///
    /// A front-end can read this before offering an edit UI so it can explain the situation up
    /// front rather than after a refused write.
    pub fn load_error(&self, scope: SettingsScope) -> Option<&str> {
        match scope {
            SettingsScope::Global => self.global_load_error.as_deref(),
            SettingsScope::Project => self.project_load_error.as_deref(),
        }
    }

    /// Refuse a write into a scope whose last load failed (CFG-001).
    ///
    /// Every writer opens with this, mirroring Pi's `save()` / `saveProjectSettings()` guards
    /// (settings-manager.ts ≈:614-628 / ≈:633-646). Without it a single typo — a trailing comma, an
    /// unclosed brace — turns the very next `/config` toggle, `/theme`, analytics opt-in, or
    /// `set_editor_padding_x` into a total rewrite of the file as `{"<key>": <value>}`, silently
    /// destroying every other setting the user had.
    fn ensure_scope_writable(&self, scope: SettingsScope) -> Result<(), ConfigError> {
        match self.load_error(scope) {
            Some(message) => Err(ConfigError::SettingsWriteRefused {
                scope,
                message: message.to_string(),
            }),
            None => Ok(()),
        }
    }

    /// Persist a single field via scoped read-modify-write that re-reads the on-disk file and
    /// applies only the modified field (concurrent-edit safe; R-07-004). Project writes require
    /// trust.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::Untrusted`] for a project write in an untrusted folder.
    /// - [`ConfigError::SettingsWriteRefused`] if that scope's file failed to load, or if it became
    ///   unparseable between the load and this write — the file is left byte-for-byte unchanged
    ///   (CFG-001).
    pub fn set<T: serde::Serialize>(
        &mut self,
        scope: SettingsScope,
        key: &str,
        value: T,
    ) -> Result<(), ConfigError> {
        if scope == SettingsScope::Project && !self.project_trusted {
            return Err(ConfigError::Untrusted);
        }
        self.ensure_scope_writable(scope)?;
        let json = serde_json::to_value(value)?;
        let key_owned = key.to_string();
        let mut corrupt: Option<String> = None;
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                // Absent file: create it. This is the ONLY branch that may start from an empty doc.
                None => Settings::default(),
                // Corruption that appeared BETWEEN the load and this locked write. Returning `None`
                // leaves the file untouched; the message is surfaced below so the caller can tell
                // the write did not happen (CFG-001).
                Some(Err(e)) => {
                    corrupt = Some(format!("parse error: {e}"));
                    return None;
                }
            };
            // CFG-062 — "clear" means the key is GONE, not present-and-null. Pi's clearing setters
            // assign `undefined` (`setShellPath` settings-manager.ts:883-887, `setShellCommandPrefix`
            // :914-918, `setNpmCommand` :924-928 @v0.83.0) and `persistScopedSettings` serializes
            // through `JSON.stringify(mergedSettings, null, 2)` (:605), which OMITS
            // undefined-valued properties. `serde_json` has no `undefined`, so `None::<String>`
            // arrives here as `Value::Null` and used to persist as `"shellPath": null` — a value
            // upstream cannot write, and one that a lower layer's `deep_merge` treats as a real
            // override (both sides let a project `null` blank a global value, so the divergence is
            // the WRITE, not the merge).
            if json.is_null() {
                doc.obj.remove(&key_owned);
            } else {
                doc.obj.insert(key_owned.clone(), json.clone());
            }
            Some(doc.to_pretty())
        })?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused { scope, message });
        }
        self.reload_internal();
        Ok(())
    }

    /// Persist a nested field (e.g. `terminal.showImages`) via scoped read-modify-write, creating
    /// intermediate objects and PRESERVING sibling nested keys (Pi `persistScopedSettings` nested
    /// tracking, settings-manager.ts:573-602). Unlike [`Self::set`], this never clobbers the rest of
    /// the parent object. Project writes require trust.
    ///
    /// # Errors
    ///
    /// Same as [`Self::set`], including the [`ConfigError::SettingsWriteRefused`] refusal that keeps
    /// an unparseable file intact (CFG-001).
    pub fn set_nested(
        &mut self,
        scope: SettingsScope,
        path: &[&str],
        value: Value,
    ) -> Result<(), ConfigError> {
        if path.is_empty() {
            return Ok(());
        }
        if scope == SettingsScope::Project && !self.project_trusted {
            return Err(ConfigError::Untrusted);
        }
        self.ensure_scope_writable(scope)?;
        let path_owned: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        let mut corrupt: Option<String> = None;
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                None => Settings::default(),
                Some(Err(e)) => {
                    corrupt = Some(format!("parse error: {e}"));
                    return None;
                }
            };
            set_value_at_path(&mut doc.obj, &path_owned, value.clone());
            Some(doc.to_pretty())
        })?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused { scope, message });
        }
        self.reload_internal();
        Ok(())
    }

    /// Persist a nested field to the on-disk store via scoped read-modify-write **without** updating
    /// the in-memory merged view (an additive `&self` write seam: `set_nested` requires `&mut self`
    /// because it reloads, but a front-end holding the manager behind an `Arc` — the TUI `/config`
    /// selector — drives a `/reload` afterward, exactly as Pi's settings selector applies-then-reloads,
    /// settings-manager.ts:573). The change becomes visible in `effective()` after the next
    /// [`Self::reload`]. Project writes still require trust (R-07-004).
    ///
    /// # Errors
    ///
    /// Same as [`Self::set`], including the [`ConfigError::SettingsWriteRefused`] refusal that keeps
    /// an unparseable file intact (CFG-001). This is the seam the TUI `/config` selector and the
    /// `cyrup config` subcommand drive, and both already surface the returned error to the user.
    pub fn persist_nested(
        &self,
        scope: SettingsScope,
        path: &[&str],
        value: Value,
    ) -> Result<(), ConfigError> {
        if path.is_empty() {
            return Ok(());
        }
        if scope == SettingsScope::Project && !self.project_trusted {
            return Err(ConfigError::Untrusted);
        }
        self.ensure_scope_writable(scope)?;
        let path_owned: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        let mut corrupt: Option<String> = None;
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                None => Settings::default(),
                Some(Err(e)) => {
                    corrupt = Some(format!("parse error: {e}"));
                    return None;
                }
            };
            set_value_at_path(&mut doc.obj, &path_owned, value.clone());
            Some(doc.to_pretty())
        })?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused { scope, message });
        }
        Ok(())
    }

    /// `setMermaidRenderingMode` (Pi settings-manager.ts:1257-1262 @v0.84.1): writes the GLOBAL
    /// scope through `markdown.mermaid`, so a sibling `markdown.codeBlockIndent` survives — pi does
    /// `this.globalSettings.markdown ??= {}` and assigns one key, never replacing the block.
    /// CFG-040.
    pub fn set_mermaid_rendering_mode(
        &mut self,
        mode: MermaidRenderingMode,
    ) -> Result<(), ConfigError> {
        self.set_nested(
            SettingsScope::Global,
            &["markdown", "mermaid"],
            Value::String(mode.as_str().to_string()),
        )
    }

    /// `setEditorPaddingX`: clamp to 0..=3 (Pi settings-manager.ts:1179-1183).
    pub fn set_editor_padding_x(&mut self, padding: f64) -> Result<(), ConfigError> {
        let clamped = (padding.floor() as i64).clamp(0, 3);
        self.set(SettingsScope::Global, "editorPaddingX", clamped)
    }

    /// `setAutocompleteMaxVisible`: clamp to 3..=20 (Pi settings-manager.ts:1189-1193).
    pub fn set_autocomplete_max_visible(&mut self, max_visible: f64) -> Result<(), ConfigError> {
        let clamped = (max_visible.floor() as i64).clamp(3, 20);
        self.set(SettingsScope::Global, "autocompleteMaxVisible", clamped)
    }

    /// `setImageWidthCells`: floor and clamp to >=1 (Pi settings-manager.ts:1068-1075).
    pub fn set_image_width_cells(&mut self, width: f64) -> Result<(), ConfigError> {
        let clamped = (width.floor() as i64).max(1);
        self.set_nested(
            SettingsScope::Global,
            &["terminal", "imageWidthCells"],
            clamped.into(),
        )
    }

    /// `setHttpIdleTimeoutMs`: reject non-finite/negative; floor (Pi settings-manager.ts:820-827).
    pub fn set_http_idle_timeout_ms(&mut self, timeout_ms: f64) -> Result<(), ConfigError> {
        if !timeout_ms.is_finite() || timeout_ms < 0.0 {
            return Err(ConfigError::InvalidSetting {
                key: "httpIdleTimeoutMs".to_string(),
                value: timeout_ms.to_string(),
            });
        }
        self.set(
            SettingsScope::Global,
            "httpIdleTimeoutMs",
            timeout_ms.floor() as i64,
        )
    }

    /// `setShowImages` (nested `terminal.showImages`; Pi settings-manager.ts:1051-1058).
    pub fn set_show_images(&mut self, show: bool) -> Result<(), ConfigError> {
        self.set_nested(
            SettingsScope::Global,
            &["terminal", "showImages"],
            show.into(),
        )
    }

    /// `setEnableAnalytics`: set the opt-in flag and, on first opt-in, generate a `trackingId`
    /// (randomUUID) if absent (Pi settings-manager.ts:943-951). Both fields land in one write.
    ///
    /// Does not route through [`Self::set`] (it writes two keys under one lock), so it carries its
    /// own copy of the CFG-001 guard.
    pub fn set_enable_analytics(&mut self, enabled: bool) -> Result<(), ConfigError> {
        self.ensure_scope_writable(SettingsScope::Global)?;
        let mut corrupt: Option<String> = None;
        self.store
            .with_lock(SettingsScope::Global, &mut |current| {
                let mut doc = match current.map(Settings::parse) {
                    Some(Ok(s)) => s,
                    None => Settings::default(),
                    Some(Err(e)) => {
                        corrupt = Some(format!("parse error: {e}"));
                        return None;
                    }
                };
                doc.obj
                    .insert("enableAnalytics".to_string(), Value::Bool(enabled));
                let has_tracking = doc
                    .obj
                    .get("trackingId")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty());
                if enabled && !has_tracking {
                    doc.obj
                        .insert("trackingId".to_string(), Value::String(random_uuid_v4()));
                }
                Some(doc.to_pretty())
            })?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused {
                scope: SettingsScope::Global,
                message,
            });
        }
        self.reload_internal();
        Ok(())
    }

    pub fn drain_load_errors(&mut self) -> Vec<ScopedError> {
        std::mem::take(&mut self.load_errors)
    }
}

/// Set `value` at a nested object path, creating intermediate objects and replacing any
/// non-object on the way (Pi nested setters create `{}` then assign).
fn set_value_at_path(map: &mut Map<String, Value>, path: &[String], value: Value) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        // CFG-062, nested leg. `persistScopedSettings` writes the nested object through the same
        // `JSON.stringify(mergedSettings, null, 2)` (settings-manager.ts:605 @v0.83.0), so an
        // undefined nested field is omitted at depth exactly as a top-level one is. A `Null` leaf
        // therefore CLEARS the key rather than persisting `"terminal": { "showImages": null }`.
        if value.is_null() {
            map.remove(first);
        } else {
            map.insert(first.clone(), value);
        }
        return;
    }
    let entry = map
        .entry(first.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Value::Object(child) = entry {
        set_value_at_path(child, rest, value);
    }
}

/// Generate a random v4 UUID string (Pi `randomUUID()` for `trackingId`). Dependency-free: derives
/// 16 entropy bytes from the OS-seeded `RandomState` hasher plus monotonic/PID inputs, then sets the
/// RFC 4122 version (4) and variant bits. Used only for a non-secret analytics tracking id.
fn random_uuid_v4() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let entropy = |salt: usize| -> u64 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(std::process::id() as u64);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        hasher.write_usize(salt);
        hasher.finish()
    };

    let mut bytes = [0u8; 16];
    for (i, b) in entropy(0)
        .to_le_bytes()
        .into_iter()
        .chain(entropy(1).to_le_bytes())
        .enumerate()
    {
        if let Some(slot) = bytes.get_mut(i) {
            *slot = b;
        }
    }
    // RFC 4122: version 4, variant 10xx.
    if let Some(b) = bytes.get_mut(6) {
        *b = (*b & 0x0f) | 0x40;
    }
    if let Some(b) = bytes.get_mut(8) {
        *b = (*b & 0x3f) | 0x80;
    }
    let mut s = String::with_capacity(36);
    for (i, b) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}
