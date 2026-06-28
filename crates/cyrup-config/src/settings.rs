//! Layered settings: global ◁ project ◁ CLI deep-merge with unknown-key preservation
//! (arch-07 §3.2/§3.3/§4.3, R-07-001/004/005).
//!
//! Settings are represented structurally as a JSON object map. This makes unknown-key
//! preservation (R-07-004) and per-key nested deep-merge (R-07-001) trivially correct, while
//! typed getters apply documented defaults in one place (mirrors Pi's `getX()` accessors).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::ThinkingLevel;
use serde_json::{Map, Value};

use crate::error::{ConfigError, ScopedError};

/// Which layer a settings document belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsScope {
    Global,
    Project,
}

/// `defaultProjectTrust` (global-only; §4.8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefaultProjectTrust {
    #[default]
    Ask,
    Always,
    Never,
}

/// A key that is only honoured in the GLOBAL scope; stripped from project/CLI before merge.
const GLOBAL_ONLY_KEYS: &[&str] = &["defaultProjectTrust"];

/// A settings document (one scope). Wraps a JSON object so unknown / not-yet-modelled keys are
/// preserved across a load→save round-trip (R-07-004).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    obj: Map<String, Value>,
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse from raw JSON text. An empty / whitespace document is the empty object.
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let value: Value = serde_json::from_str(text)?;
        match value {
            Value::Object(obj) => Ok(Self { obj }),
            // A non-object top-level is treated as empty (degraded), never a panic.
            _ => Ok(Self::default()),
        }
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
        self.obj.insert(key.to_string(), serde_json::to_value(value)?);
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.obj.get(key)
    }

    fn get_str(&self, key: &str) -> Option<String> {
        self.obj.get(key).and_then(Value::as_str).map(str::to_string)
    }

    fn get_bool(&self, key: &str) -> Option<bool> {
        self.obj.get(key).and_then(Value::as_bool)
    }
}

/// Remove keys that are only honoured globally (§4.8: `defaultProjectTrust`).
fn strip_global_only(settings: &mut Settings) {
    for k in GLOBAL_ONLY_KEYS {
        settings.obj.remove(*k);
    }
}

/// Deep-merge `over` onto `base` (R-07-001): objects merge recursively per-key; primitives and
/// arrays replace wholesale (matching Pi).
pub fn deep_merge(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            let mut out = b.clone();
            for (k, ov) in o {
                let merged = match out.get(k) {
                    Some(bv) => deep_merge(bv, ov),
                    None => ov.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        // arrays + primitives: `over` wins
        (_, over) => over.clone(),
    }
}

/// The merged, read-only effective view (R-07-001). Getters apply documented defaults.
#[derive(Clone, Debug, Default)]
pub struct EffectiveSettings {
    merged: Settings,
}

impl EffectiveSettings {
    pub fn from_settings(merged: Settings) -> Self {
        Self { merged }
    }

    pub fn raw(&self) -> &Settings {
        &self.merged
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.merged.get(key)
    }

    pub fn default_provider(&self) -> Option<String> {
        self.merged.get_str("defaultProvider")
    }

    pub fn default_model(&self) -> Option<String> {
        self.merged.get_str("defaultModel")
    }

    pub fn default_thinking_level(&self) -> ThinkingLevel {
        self.merged
            .get("defaultThinkingLevel")
            .and_then(|v| serde_json::from_value::<ThinkingLevel>(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn hide_thinking_block(&self) -> bool {
        self.merged.get_bool("hideThinkingBlock").unwrap_or(false)
    }

    pub fn theme(&self) -> String {
        self.merged.get_str("theme").unwrap_or_else(|| "dark".to_string())
    }

    /// `defaultProjectTrust` is global-only; stripped from project/cli before merge.
    pub fn default_project_trust(&self) -> DefaultProjectTrust {
        self.merged
            .get("defaultProjectTrust")
            .and_then(|v| serde_json::from_value::<DefaultProjectTrust>(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn external_editor(&self, env: &crate::env::EnvVars) -> String {
        self.merged
            .get_str("externalEditor")
            .or_else(|| env.visual.clone())
            .or_else(|| env.editor.clone())
            .unwrap_or_else(|| if cfg!(windows) { "notepad".to_string() } else { "nano".to_string() })
    }

    pub fn enable_install_telemetry(&self) -> bool {
        self.merged.get_bool("enableInstallTelemetry").unwrap_or(true)
    }

    pub fn enable_analytics(&self) -> bool {
        self.merged.get_bool("enableAnalytics").unwrap_or(false)
    }

    pub fn enabled_models(&self) -> Vec<String> {
        self.merged
            .get("enabledModels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub fn session_dir(&self) -> Option<String> {
        self.merged.get_str("sessionDir")
    }
}

/// Serialized read-modify-write of one scope's raw JSON text (arch-07 §3.3).
pub trait SettingsStore: Send + Sync {
    /// Read the current raw text for a scope (`None` if absent).
    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError>;

    /// Serialized read-modify-write. `f` receives the current text (None if absent) and returns
    /// `Some(new)` to write or `None` to leave untouched.
    fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut dyn FnMut(Option<&str>) -> Option<String>,
    ) -> Result<(), ConfigError>;
}

/// File-backed store with a cross-process advisory lock (arch-07 §5).
pub struct FileSettingsStore {
    global_path: PathBuf,
    project_path: PathBuf,
}

impl FileSettingsStore {
    pub fn new(global_path: PathBuf, project_path: PathBuf) -> Self {
        Self { global_path, project_path }
    }

    fn path(&self, scope: SettingsScope) -> &Path {
        match scope {
            SettingsScope::Global => &self.global_path,
            SettingsScope::Project => &self.project_path,
        }
    }
}

impl SettingsStore for FileSettingsStore {
    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError> {
        match std::fs::read_to_string(self.path(scope)) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ConfigError::Io(e)),
        }
    }

    fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut dyn FnMut(Option<&str>) -> Option<String>,
    ) -> Result<(), ConfigError> {
        let path = self.path(scope).to_path_buf();
        let _guard = crate::lock::FileLock::acquire(&path)?;
        let current = match std::fs::read_to_string(&path) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(ConfigError::Io(e)),
        };
        if let Some(new_text) = f(current.as_deref()) {
            crate::lock::write_atomic(&path, new_text.as_bytes(), false)?;
        }
        Ok(())
    }
}

/// In-memory store for tests / non-persistent runs.
#[derive(Default)]
pub struct InMemorySettingsStore {
    global: std::sync::Mutex<Option<String>>,
    project: std::sync::Mutex<Option<String>>,
}

impl InMemorySettingsStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, scope: SettingsScope) -> &std::sync::Mutex<Option<String>> {
        match scope {
            SettingsScope::Global => &self.global,
            SettingsScope::Project => &self.project,
        }
    }

    pub fn seed(&self, scope: SettingsScope, text: &str) {
        if let Ok(mut g) = self.slot(scope).lock() {
            *g = Some(text.to_string());
        }
    }
}

impl SettingsStore for InMemorySettingsStore {
    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError> {
        Ok(self.slot(scope).lock().ok().and_then(|g| g.clone()))
    }

    fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut dyn FnMut(Option<&str>) -> Option<String>,
    ) -> Result<(), ConfigError> {
        let mut guard =
            self.slot(scope).lock().map_err(|_| ConfigError::Trust("poisoned lock".into()))?;
        if let Some(new) = f(guard.as_deref()) {
            *guard = Some(new);
        }
        Ok(())
    }
}

/// The layered settings facade (arch-07 §3.3). Holds the three layers + a memoized merge.
pub struct SettingsManager {
    store: Arc<dyn SettingsStore>,
    global: Settings,
    project: Settings,
    cli: Settings,
    effective: EffectiveSettings,
    project_trusted: bool,
    load_errors: Vec<ScopedError>,
}

impl SettingsManager {
    /// Load global unconditionally; load project ONLY if `project_trusted` (R-07-002). A parse
    /// error degrades that scope to empty and records a `ScopedError` (R-00-009).
    pub fn load(store: Arc<dyn SettingsStore>, cli: Settings, project_trusted: bool) -> Self {
        let mut mgr = Self {
            store,
            global: Settings::default(),
            project: Settings::default(),
            cli,
            effective: EffectiveSettings::default(),
            project_trusted,
            load_errors: Vec::new(),
        };
        mgr.reload_internal();
        mgr
    }

    fn load_scope(&mut self, scope: SettingsScope) -> Settings {
        match self.store.read(scope) {
            Ok(Some(text)) => match Settings::parse(&text) {
                Ok(s) => s,
                Err(e) => {
                    self.load_errors.push(ScopedError {
                        scope,
                        message: format!("parse error: {e}"),
                    });
                    Settings::default()
                }
            },
            Ok(None) => Settings::default(),
            Err(e) => {
                self.load_errors.push(ScopedError { scope, message: e.to_string() });
                Settings::default()
            }
        }
    }

    fn reload_internal(&mut self) {
        self.global = self.load_scope(SettingsScope::Global);
        self.project = if self.project_trusted {
            self.load_scope(SettingsScope::Project)
        } else {
            Settings::default()
        };
        self.recompute();
    }

    fn recompute(&mut self) {
        // Strip global-only keys from project + cli before merge.
        let mut project = self.project.clone();
        let mut cli = self.cli.clone();
        strip_global_only(&mut project);
        strip_global_only(&mut cli);

        let merged = deep_merge(&self.global.to_value(), &project.to_value());
        let merged = deep_merge(&merged, &cli.to_value());
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

    /// Trust transition: false→true loads the project scope; true→false drops it (R-07-002/012).
    pub fn set_project_trusted(&mut self, trusted: bool) {
        if self.project_trusted != trusted {
            self.project_trusted = trusted;
            self.reload_internal();
        }
    }

    /// Persist a single field via scoped read-modify-write that re-reads the on-disk file and
    /// applies only the modified field (concurrent-edit safe; R-07-004). Project writes require
    /// trust.
    pub fn set<T: serde::Serialize>(
        &mut self,
        scope: SettingsScope,
        key: &str,
        value: T,
    ) -> Result<(), ConfigError> {
        if scope == SettingsScope::Project && !self.project_trusted {
            return Err(ConfigError::Untrusted);
        }
        let json = serde_json::to_value(value)?;
        let key_owned = key.to_string();
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                _ => Settings::default(),
            };
            doc.obj.insert(key_owned.clone(), json.clone());
            Some(doc.to_pretty())
        })?;
        self.reload_internal();
        Ok(())
    }

    pub fn drain_load_errors(&mut self) -> Vec<ScopedError> {
        std::mem::take(&mut self.load_errors)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

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
        assert_eq!(reparsed.get("someFutureKey"), Some(&serde_json::json!({"a": 1})));
        assert_eq!(reparsed.get("topUnknown"), Some(&serde_json::json!(7)));
        assert_eq!(reparsed.get("defaultModel"), Some(&serde_json::json!("x")));
    }

    #[test]
    fn cli_then_project_then_global_resolution() {
        // A-07-1
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "defaultModel": "g", "theme": "light" }"#);
        store.seed(SettingsScope::Project, r#"{ "defaultModel": "p" }"#);
        let mut cli = Settings::new();
        cli.set_field("defaultModel", "c").unwrap();

        let mgr = SettingsManager::load(store.clone(), cli, true);
        assert_eq!(mgr.effective().default_model(), Some("c".to_string()));
        assert_eq!(mgr.effective().theme(), "light");

        // without CLI, project wins
        let mgr2 = SettingsManager::load(store, Settings::new(), true);
        assert_eq!(mgr2.effective().default_model(), Some("p".to_string()));
    }

    #[test]
    fn project_not_loaded_until_trusted() {
        // R-07-002
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "defaultModel": "g" }"#);
        store.seed(SettingsScope::Project, r#"{ "defaultModel": "p" }"#);

        let mut mgr = SettingsManager::load(store, Settings::new(), false);
        assert_eq!(mgr.effective().default_model(), Some("g".to_string()));
        mgr.set_project_trusted(true);
        assert_eq!(mgr.effective().default_model(), Some("p".to_string()));
        mgr.set_project_trusted(false);
        assert_eq!(mgr.effective().default_model(), Some("g".to_string()));
    }

    #[test]
    fn default_project_trust_is_global_only() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "defaultProjectTrust": "always" }"#);
        // project tries to set it but it must be stripped
        store.seed(SettingsScope::Project, r#"{ "defaultProjectTrust": "never" }"#);
        let mgr = SettingsManager::load(store, Settings::new(), true);
        assert_eq!(mgr.effective().default_project_trust(), DefaultProjectTrust::Always);
    }

    #[test]
    fn set_field_preserves_unknown_keys() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "futureKey": 42, "defaultModel": "old" }"#);
        let mut mgr = SettingsManager::load(store.clone(), Settings::new(), false);
        mgr.set(SettingsScope::Global, "defaultModel", "new").unwrap();
        let raw = store.read(SettingsScope::Global).unwrap().unwrap();
        let s = Settings::parse(&raw).unwrap();
        assert_eq!(s.get("futureKey"), Some(&serde_json::json!(42)));
        assert_eq!(s.get("defaultModel"), Some(&serde_json::json!("new")));
    }

    #[test]
    fn project_write_requires_trust() {
        let store = Arc::new(InMemorySettingsStore::new());
        let mut mgr = SettingsManager::load(store, Settings::new(), false);
        let err = mgr.set(SettingsScope::Project, "defaultModel", "x");
        assert!(matches!(err, Err(ConfigError::Untrusted)));
    }
}
