//! The extension config `config.json` (port of pi `extension-config.ts`). Two toggles (`debug`,
//! `yoloMode`) plus the forwarded-prompt timeout. JSONC (trailing commas / comments allowed).
//!
//! Ports pi's `ensurePermissionSystemConfig` + `loadPermissionSystemConfig`
//! (`extension-config.ts:91-130`): a missing config file is auto-materialized on disk (pretty
//! `DEFAULT_EXTENSION_CONFIG` + trailing newline, parent dir created recursively) rather than only
//! defaulted in memory; the config path is overridable via
//! `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` (pi's `PI_PERMISSION_SYSTEM_CONFIG_PATH` /
//! `CONFIG_PATH_ENV_KEY`, `:40-46`), taking precedence over the caller-supplied default path (pi's
//! `configPath || overridePath || CONFIG_PATH` — the caller here never supplies an explicit
//! override, so this reduces to `env || default`); and a load failure is only silenced when it's
//! the expected "file absent" case (`formatJsoncConfigLoadWarning`'s ENOENT check,
//! `jsonc-config.ts:37-52`) — any other read/parse failure produces a warning (pi surfaces it via
//! `ctx.ui.notify`; wiring a host-UI channel here is out of scope for this module, so it is
//! `eprintln!`ed and also returned structurally via [`ExtensionConfigLoadResult`] for a future
//! caller to surface).

use std::path::{Path, PathBuf};

use crate::jsonc;

/// pi `CONFIG_PATH_ENV_KEY` (`extension-config.ts:40`), renamed to this crate's `CYRUP_` env-var
/// convention (see `extension.rs::INSTALL_ENV_VAR`, `forwarding.rs::FORWARDING_AGENT_DIR_ENV`).
pub const CONFIG_PATH_ENV_KEY: &str = "CYRUP_PERMISSION_SYSTEM_CONFIG_PATH";

/// pi `PermissionSystemExtensionConfig` (`extension-config.ts:10-14`); defaults `{false, false,
/// Some(30)}` (`:27-31`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionConfig {
    /// pi `debug` — `strict === true` (`:81`).
    pub debug: bool,
    /// pi `yoloMode` — `strict === true` (`:82`). (Auto-approve of `ask` is P-1/P-3 territory; carried
    /// now for shape parity.)
    pub yolo_mode: bool,
    /// pi `forwardedPromptTimeoutSeconds`: `null`/`false` → `None` (indefinite); a finite `> 0`
    /// number → that; else `Some(30)` (`:74-78`). Consumed by forwarding (P-4).
    pub forwarded_prompt_timeout_seconds: Option<u64>,
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        // pi `DEFAULT_EXTENSION_CONFIG` (`extension-config.ts:27-31`).
        ExtensionConfig { debug: false, yolo_mode: false, forwarded_prompt_timeout_seconds: Some(30) }
    }
}

/// pi `PermissionSystemConfigLoadResult` (`extension-config.ts:16-20`): the config plus whether
/// this call materialized the default file on disk and, if the load wasn't clean, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionConfigLoadResult {
    pub config: ExtensionConfig,
    pub created: bool,
    pub warning: Option<String>,
}

/// Outcome of the disk-materialization step (pi `ensurePermissionSystemConfig`'s return shape,
/// `extension-config.ts:91-107`).
struct EnsureResult {
    created: bool,
    warning: Option<String>,
}

impl ExtensionConfig {
    /// pi `getPermissionSystemConfigPath(configPath?)` (`extension-config.ts:43-46`):
    /// `configPath || overridePath || CONFIG_PATH`. This crate's call site always supplies a
    /// computed default path (the analog of pi's `CONFIG_PATH`) rather than an optional explicit
    /// override, so the precedence collapses to `env (trimmed, non-empty) || default_path`.
    #[must_use]
    pub fn resolve_config_path(default_path: &Path) -> PathBuf {
        std::env::var(CONFIG_PATH_ENV_KEY)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| default_path.to_path_buf())
    }

    /// Load from `path` (JSONC), applying the env-var override and disk-materialization pi
    /// performs before every load. Kept as a plain `ExtensionConfig` return for source
    /// compatibility with the existing call site; use [`ExtensionConfig::load_with_result`] for
    /// the structured `{config, created, warning}` pi actually returns.
    #[must_use]
    pub fn load(path: &Path) -> ExtensionConfig {
        Self::load_with_result(path).config
    }

    /// pi `loadPermissionSystemConfig(configPath?)` (`extension-config.ts:109-130`): resolve the
    /// path, `ensurePermissionSystemConfig` it onto disk if absent, then read + parse + normalize,
    /// falling back to defaults with a warning (ENOENT-suppressed) on any failure.
    #[must_use]
    pub fn load_with_result(path: &Path) -> ExtensionConfigLoadResult {
        let resolved = Self::resolve_config_path(path);
        let ensure = Self::ensure_on_disk(&resolved);

        match std::fs::read_to_string(&resolved) {
            Ok(text) => {
                let subject = "permission-system config";
                let path_str = resolved.display().to_string();
                match jsonc::parse_config(&text, &path_str, subject) {
                    Ok(value) => ExtensionConfigLoadResult {
                        config: Self::normalize(&value),
                        created: ensure.created,
                        warning: ensure.warning,
                    },
                    Err(err) => {
                        // pi `formatJsoncConfigLoadWarning(configPath, error, subject,
                        // "using default extension config")` (`jsonc-config.ts:37-52`): a parse
                        // error is never an ENOENT, so it always gets the fallback suffix appended
                        // — unless `ensureResult.warning` already won (`extension-config.ts:125`).
                        let warning =
                            ensure.warning.unwrap_or_else(|| format!("{err}; using default extension config."));
                        eprintln!("cyrup-permission-system: warning: {warning}");
                        ExtensionConfigLoadResult {
                            config: ExtensionConfig::default(),
                            created: ensure.created,
                            warning: Some(warning),
                        }
                    }
                }
            }
            Err(err) => {
                // pi's ENOENT-suppression (`isNodeErrorWithCode(error, "ENOENT")` →
                // `formatJsoncConfigLoadWarning` returns `null`): the "file absent" case is
                // expected/silent. Any other read failure (permission denied, EISDIR, ...) is
                // surfaced. `ensureResult.warning` (a failed materialize-to-disk) always wins over
                // either outcome (`extension-config.ts:125`).
                let warning = ensure.warning.or_else(|| {
                    if err.kind() == std::io::ErrorKind::NotFound {
                        None
                    } else {
                        Some(format!(
                            "Failed to load {subject} at '{path}': {err}; using default extension config.",
                            subject = "permission-system config",
                            path = resolved.display()
                        ))
                    }
                });
                if let Some(ref w) = warning {
                    eprintln!("cyrup-permission-system: warning: {w}");
                }
                ExtensionConfigLoadResult { config: ExtensionConfig::default(), created: ensure.created, warning }
            }
        }
    }

    /// pi `ensurePermissionSystemConfig(configPath)` (`extension-config.ts:91-107`): if `path`
    /// doesn't already exist, `mkdir -p` its parent and write the pretty-printed default config +
    /// trailing newline, leaving a real, editable template file on disk.
    fn ensure_on_disk(path: &Path) -> EnsureResult {
        if path.exists() {
            return EnsureResult { created: false, warning: None };
        }

        let write_result: std::io::Result<()> = (|| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, Self::default_config_content())
        })();

        match write_result {
            Ok(()) => EnsureResult { created: true, warning: None },
            Err(err) => EnsureResult {
                created: false,
                warning: Some(format!(
                    "Failed to initialize permission-system config at '{}': {err}",
                    path.display()
                )),
            },
        }
    }

    /// pi `createDefaultConfigContent()` (`extension-config.ts:65-67`):
    /// `` `${JSON.stringify(DEFAULT_EXTENSION_CONFIG, null, 2)}\n` `` — field order/spacing built
    /// by hand (rather than via `serde_json`, whose default `Map` is alphabetically ordered) so the
    /// on-disk template matches pi's `debug`/`yoloMode`/`forwardedPromptTimeoutSeconds` order byte
    /// for byte.
    fn default_config_content() -> String {
        let default = ExtensionConfig::default();
        let timeout = match default.forwarded_prompt_timeout_seconds {
            Some(seconds) => seconds.to_string(),
            None => "null".to_string(),
        };
        format!(
            "{{\n  \"debug\": {},\n  \"yoloMode\": {},\n  \"forwardedPromptTimeoutSeconds\": {timeout}\n}}\n",
            default.debug, default.yolo_mode
        )
    }

    /// pi `normalizePermissionSystemConfig` (`extension-config.ts:69-85`).
    #[must_use]
    pub fn normalize(value: &serde_json::Value) -> ExtensionConfig {
        let default = ExtensionConfig::default();
        let debug = value.get("debug").and_then(serde_json::Value::as_bool).unwrap_or(false);
        let yolo_mode = value.get("yoloMode").and_then(serde_json::Value::as_bool).unwrap_or(false);

        let forwarded = match value.get("forwardedPromptTimeoutSeconds") {
            // `null` / `false` → indefinite.
            Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Bool(false)) => None,
            Some(v) => match v.as_f64() {
                // finite, > 0 → that value (floored to whole seconds).
                Some(n) if n.is_finite() && n > 0.0 => Some(n as u64),
                _ => default.forwarded_prompt_timeout_seconds,
            },
            None => default.forwarded_prompt_timeout_seconds,
        };

        ExtensionConfig { debug, yolo_mode, forwarded_prompt_timeout_seconds: forwarded }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::sync::{Mutex, OnceLock};

    use super::*;

    /// Guards tests that mutate `CONFIG_PATH_ENV_KEY` (process-wide state) from running
    /// concurrently with each other.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn absent_is_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.json");
        assert_eq!(ExtensionConfig::load(&path), ExtensionConfig::default());
    }

    // Regression test for pi `ensurePermissionSystemConfig` (`extension-config.ts:91-107`):
    // pre-fix, `load` never wrote anything to disk on a missing config, so `config.json` never
    // existed unless something external created it. Loading an absent path must now materialize a
    // real, editable default-config template file at that path (mkdir -p'ing the parent first).
    #[test]
    fn absent_config_is_materialized_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        assert!(!path.exists());

        let result = ExtensionConfig::load_with_result(&path);

        assert!(result.created, "first load of an absent config must report created: true");
        assert!(result.warning.is_none());
        assert_eq!(result.config, ExtensionConfig::default());
        assert!(path.exists(), "config.json must now exist on disk");

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "{\n  \"debug\": false,\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n");

        // A second load must see the just-created file rather than re-creating it.
        let second = ExtensionConfig::load_with_result(&path);
        assert!(!second.created);
        assert!(second.warning.is_none());
    }

    #[test]
    fn null_timeout_is_indefinite_and_bools_parse() {
        let v = serde_json::json!({"debug": true, "yoloMode": true, "forwardedPromptTimeoutSeconds": null});
        let c = ExtensionConfig::normalize(&v);
        assert!(c.debug && c.yolo_mode);
        assert_eq!(c.forwarded_prompt_timeout_seconds, None);
    }

    #[test]
    fn finite_positive_timeout_kept_else_default() {
        assert_eq!(
            ExtensionConfig::normalize(&serde_json::json!({"forwardedPromptTimeoutSeconds": 45}))
                .forwarded_prompt_timeout_seconds,
            Some(45)
        );
        assert_eq!(
            ExtensionConfig::normalize(&serde_json::json!({"forwardedPromptTimeoutSeconds": -5}))
                .forwarded_prompt_timeout_seconds,
            Some(30)
        );
    }

    // Regression test for pi `formatJsoncConfigLoadWarning` (`jsonc-config.ts:37-52`) as used by
    // `loadPermissionSystemConfig` (`extension-config.ts:121-129`): a malformed-but-present config
    // must produce a warning shaped like pi's (`Failed to parse ... at '...' (...); using default
    // extension config.`), not the old bespoke "is not valid config JSON" message, and must be
    // returned structurally (not only `eprintln!`ed) so a caller can surface it.
    #[test]
    fn malformed_present_config_warns_like_pi_and_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{not json").unwrap();

        let result = ExtensionConfig::load_with_result(&path);

        assert_eq!(result.config, ExtensionConfig::default());
        assert!(!result.created, "a pre-existing file must not be reported as created");
        let warning = result.warning.expect("malformed JSON must produce a warning");
        assert!(
            warning.starts_with("Failed to parse permission-system config at"),
            "unexpected warning: {warning}"
        );
        assert!(warning.ends_with("using default extension config."), "unexpected warning: {warning}");
    }

    // Regression test for pi's ENOENT-only suppression in `formatJsoncConfigLoadWarning`
    // (`jsonc-config.ts:43-45`) vs. `formatJsoncConfigLoadWarning` on any OTHER read failure: an
    // absent file is silent (already covered by `absent_is_defaults`); a present-but-unreadable
    // file (e.g. a directory sitting at the config path) is NOT ENOENT and must produce a warning
    // instead of being silently swallowed like the pre-fix blanket `Ok(text) else return default`.
    #[test]
    fn present_but_unreadable_config_warns_instead_of_silent_default() {
        let dir = tempfile::tempdir().unwrap();
        // A directory at the config path exists (so `ensure_on_disk` does not try to create it)
        // but cannot be read as a file, giving a non-ENOENT `io::Error`.
        let path = dir.path().join("config.json");
        std::fs::create_dir(&path).unwrap();

        let result = ExtensionConfig::load_with_result(&path);

        assert_eq!(result.config, ExtensionConfig::default());
        assert!(!result.created);
        let warning = result.warning.expect("a non-ENOENT read failure must produce a warning, not silence");
        assert!(warning.contains("using default extension config."), "unexpected warning: {warning}");
    }

    // Regression test for pi `getPermissionSystemConfigPath` (`extension-config.ts:43-46`):
    // `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` (this crate's analog of pi's
    // `PI_PERMISSION_SYSTEM_CONFIG_PATH`) must override the caller-supplied default path. Pre-fix,
    // no environment variable was ever consulted anywhere in the crate.
    #[test]
    fn env_var_overrides_default_config_path() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let overridden = dir.path().join("overridden.json");
        std::fs::write(&overridden, r#"{"debug": true}"#).unwrap();
        let default_path = dir.path().join("default.json");

        // SAFETY: serialized by `env_lock` so no other test observes a partial mutation; restored
        // before returning.
        unsafe {
            std::env::set_var(CONFIG_PATH_ENV_KEY, overridden.display().to_string());
        }
        let result = ExtensionConfig::load(&default_path);
        unsafe {
            std::env::remove_var(CONFIG_PATH_ENV_KEY);
        }

        assert!(result.debug, "env-var override path must win over the caller-supplied default");
        assert!(!default_path.exists(), "the un-used default path must not be touched");
    }
}
