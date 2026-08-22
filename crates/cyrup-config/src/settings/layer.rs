//! One scope's raw settings document: the JSON-object wrapper that preserves unknown keys
//! (R-07-004), its per-layer typed accessors, and the global-only strip applied before merge.

use serde_json::{Map, Value};

use super::migrate::migrate_settings;
use super::types::PackageSource;

/// Keys that are only honoured in the GLOBAL scope; stripped from project/CLI before merge.
///
/// Upstream expresses this by reading them off the raw global document rather than the merged view
/// — `getGlobalSettings()` (settings-manager.ts:442-444 @v0.83.0) returns `this.globalSettings`,
/// not `this.settings`, so a key read through it can never be supplied by a project. Exactly two
/// production keys are read that way at v0.83.0:
///
/// - `defaultProjectTrust` (§4.8).
/// - `httpProxy` — `applyHttpProxySettings(bootstrapSettingsManager.getGlobalSettings().httpProxy)`
///   at `main.ts:537` and `applyHttpProxySettings(settingsManager.getGlobalSettings().httpProxy)`
///   at `main.ts:801`, documented as such in `packages/coding-agent/docs/settings.md:87` —
///   "HTTP proxy URL applied as `HTTP_PROXY` and `HTTPS_PROXY`. Global setting only." CFG-057: this
///   was missing, so a project `.cyrup/settings.json` could rewrite the session's egress. Note the
///   neighbouring `httpIdleTimeoutMs` IS merged upstream (`getHttpIdleTimeoutMs` reads
///   `this.settings`) — this is a per-key upstream decision, not a category.
const GLOBAL_ONLY_KEYS: &[&str] = &["defaultProjectTrust", "httpProxy"];

/// A settings document (one scope). Wraps a JSON object so unknown / not-yet-modelled keys are
/// preserved across a load→save round-trip (R-07-004).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub(crate) obj: Map<String, Value>,
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse from raw JSON text. An empty / whitespace document is the empty object. Legacy shapes
    /// are migrated on parse (R-07; Pi `migrateSettings` runs on every load path).
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        // A non-object top level is an ERROR, not a silent empty document (CFG-030). Deserializing
        // straight into the map produces serde's own "invalid type" message, which then flows
        // through `record_load_error` → the scope write latch (`ensure_scope_writable`), so the
        // next `/config` write is REFUSED instead of rewriting the user's file from `{}`. pi has no
        // degraded path either: `JSON.parse` + `migrateSettings` (settings-manager.ts:389
        // @v0.83.0), and `persistScopedSettings` (`:585-593`) spreads whatever it parsed.
        let mut obj: Map<String, Value> = serde_json::from_str(text)?;
        migrate_settings(&mut obj);
        Ok(Self { obj })
    }

    pub fn from_map(obj: Map<String, Value>) -> Self {
        Self { obj }
    }

    pub fn as_map(&self) -> &Map<String, Value> {
        &self.obj
    }

    pub fn to_value(&self) -> Value {
        Value::Object(self.obj.clone())
    }

    /// Pretty JSON (2-space) with a trailing newline (Pi byte-interop).
    pub fn to_pretty(&self) -> String {
        let mut s = serde_json::to_string_pretty(&self.obj).unwrap_or_else(|_| "{}".to_string());
        s.push('\n');
        s
    }

    pub fn is_empty(&self) -> bool {
        self.obj.is_empty()
    }

    /// Set a single top-level field (typed).
    pub fn set_field<T: serde::Serialize>(
        &mut self,
        key: &str,
        value: T,
    ) -> Result<(), serde_json::Error> {
        self.obj
            .insert(key.to_string(), serde_json::to_value(value)?);
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.obj.get(key)
    }

    pub(crate) fn get_str(&self, key: &str) -> Option<String> {
        self.obj
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    pub(crate) fn get_bool(&self, key: &str) -> Option<bool> {
        self.obj.get(key).and_then(Value::as_bool)
    }

    /// Walk a nested `a.b.c` path through objects, returning the leaf [`Value`].
    fn get_path(&self, path: &[&str]) -> Option<&Value> {
        let (first, rest) = path.split_first()?;
        let mut cur = self.obj.get(*first)?;
        for key in rest {
            cur = cur.as_object()?.get(*key)?;
        }
        Some(cur)
    }

    pub(crate) fn get_nested_bool(&self, path: &[&str]) -> Option<bool> {
        self.get_path(path).and_then(Value::as_bool)
    }

    pub(crate) fn get_nested_i64(&self, path: &[&str]) -> Option<i64> {
        self.get_path(path).and_then(Value::as_i64)
    }

    pub(crate) fn get_nested_f64(&self, path: &[&str]) -> Option<f64> {
        self.get_path(path).and_then(Value::as_f64)
    }

    pub(crate) fn get_nested_str(&self, path: &[&str]) -> Option<String> {
        self.get_path(path)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Read a `Vec<String>` settings array from THIS layer alone (no merge), with Pi's empty-array
    /// default for an absent/non-array value. Backs the merged-view list getters on
    /// [`crate::EffectiveSettings`] too, but reads a
    /// single raw layer so a caller can split global- vs project-scope resource overrides (Pi
    /// `SettingsManager` exposes the per-layer `globalSettings`/`projectSettings` split,
    /// settings-manager.ts:455-470).
    pub(crate) fn layer_string_list(&self, key: &str) -> Vec<String> {
        self.obj
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `getSkillPaths` for THIS layer only (Pi `globalSettings.skills`/`projectSettings.skills`).
    pub fn skill_paths(&self) -> Vec<String> {
        self.layer_string_list("skills")
    }

    /// `getPromptTemplatePaths` for THIS layer only.
    pub fn prompt_template_paths(&self) -> Vec<String> {
        self.layer_string_list("prompts")
    }

    /// `getThemePaths` for THIS layer only.
    pub fn theme_paths(&self) -> Vec<String> {
        self.layer_string_list("themes")
    }

    /// `getExtensionPaths` for THIS layer only. Pi's `RESOURCE_TYPES` starts with `"extensions"`
    /// (package-manager.ts:194) and `resolve()` runs `resolveLocalEntries` over all four types per
    /// scope (:905-931), so these plain paths LOAD extension roots — they are not merely filters.
    pub fn extension_paths(&self) -> Vec<String> {
        self.layer_string_list("extensions")
    }

    /// `getPackages` for THIS layer only (Pi reads `projectSettings.packages` and
    /// `globalSettings.packages` separately so the project layer wins the dedupe and can be
    /// trust-gated independently, package-manager.ts:891-898).
    ///
    /// Entries are parsed INDIVIDUALLY: one malformed entry is reported and skipped rather than
    /// discarding the whole array (which is what a blanket `from_value::<Vec<_>>().ok()` does) and
    /// never affects the rest of the settings document. Returns `(parsed, errors)`.
    pub fn packages_with_errors(&self) -> (Vec<PackageSource>, Vec<String>) {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        let Some(arr) = self.obj.get("packages").and_then(Value::as_array) else {
            // A present-but-non-array `packages` is itself worth saying out loud.
            if self.obj.contains_key("packages") {
                errors.push("settings `packages` must be an array".to_string());
            }
            return (out, errors);
        };
        for (i, v) in arr.iter().enumerate() {
            match serde_json::from_value::<PackageSource>(v.clone()) {
                Ok(p) => out.push(p),
                Err(e) => errors.push(format!(
                    "settings `packages[{i}]` is not a package source: {e}"
                )),
            }
        }
        (out, errors)
    }

    /// [`Self::packages_with_errors`] without the error channel.
    pub fn packages(&self) -> Vec<PackageSource> {
        self.packages_with_errors().0
    }
}

/// Remove keys that are only honoured globally (see [`GLOBAL_ONLY_KEYS`]).
pub(crate) fn strip_global_only(settings: &mut Settings) {
    for k in GLOBAL_ONLY_KEYS {
        settings.obj.remove(*k);
    }
}
