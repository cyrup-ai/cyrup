//! The permission-system audit / debug JSONL trail (port of pi `logging.ts:1-108` + the
//! `LOGS_DIR` / `LOGS_DIR_ENV_KEY` / `getPermissionSystemLogsDir` / `getPermissionSystemDebugPath` /
//! `ensurePermissionSystemLogsDirectory` half of `extension-config.ts:38-56,163-171`).
//!
//! pi's `createPermissionSystemLogger` exposes exactly three operations — `debug(event, details)`,
//! `review(event, details)`, `flush()` — where ONLY the `debug` stream is gated on
//! `getConfig().debug` (v0.8.0 `logging.ts:90-100`: `debug` early-returns at `:91-93`, `review` is
//! a bare `writeLine` at `:99`) and every entry is one JSON object per line, shaped
//! `{timestamp, extension, stream, event, ...details}` (`logging.ts:71-77`), appended to
//! `<logsDir>/<EXTENSION_ID>-debug.jsonl`. `logsDir` is `<extensionRoot>/logs` unless the
//! `PI_PERMISSION_SYSTEM_LOGS_DIR` env var overrides it — here
//! [`LOGS_DIR_ENV_KEY`] = `CYRUP_PERMISSION_SYSTEM_LOGS_DIR`, matching this crate's `CYRUP_` rename
//! convention (`ext_config::CONFIG_PATH_ENV_KEY`, `extension::INSTALL_ENV_VAR`).
//!
//! This is what an operator reaches for FIRST when a gate misbehaves: "why was this tool blocked",
//! "who approved this", "did the forwarded prompt time out". The `review` stream is the
//! security-relevant one (every decision the gate reaches) and is therefore ALWAYS ON; `debug`
//! carries lifecycle/diagnostic events and stays opt-in behind `config.debug`.
//!
//! `[CYRUP-DELTA]` — three documented deviations, all forced by the sync/async split:
//!
//! 1. **Serialization is a `Mutex`, not a promise chain.** pi threads every append through a single
//!    `writeQueue: Promise<void>` (`logging.ts:50-62`) so two concurrent `review()` calls can never
//!    interleave a partial line. The Rust analog of "one writer at a time" is a mutex held across
//!    the `open`+`write_all`, which gives the same guarantee without a task.
//! 2. **`flush()` is a no-op.** pi's exists because its appends are queued and outlive the call
//!    (`logging.ts:106`, awaited at `index.ts:1811,1830,1873,2437,2489`). Here the write has
//!    already hit the fd by the time `debug`/`review` returns, so there is nothing to await; the
//!    method is kept for shape parity with the upstream `PermissionSystemLogger` interface and to
//!    keep the pi call sites 1:1 recognizable.
//! 3. **An append failure IS returned as a warning.** pi cannot return it — its queued write
//!    resolves after the caller has gone, so `enqueueAppend` swallows it into
//!    `void writeQueue.catch(() => {})` (`logging.ts:57-60`) and only directory-creation /
//!    serialization failures reach the caller. The synchronous write here CAN report it, and an
//!    unwritable log directory is precisely the failure an operator needs told. It travels the
//!    SAME channel pi's other warnings do (the caller's dedup-once `reportLoggingWarning` →
//!    `ui.notify`, `index.ts:151-169`); pi's hard rule that logging never writes to stdout/stderr
//!    and never interrupts permission handling is preserved — a `Some(warning)` return is advisory
//!    and no caller branches on it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

use crate::extension::EXTENSION_ID;
use crate::forwarding::SharedExtensionConfig;

/// pi `LOGS_DIR_ENV_KEY = "PI_PERMISSION_SYSTEM_LOGS_DIR"` (`extension-config.ts:41`), renamed to
/// this crate's `CYRUP_` env-var convention.
pub const LOGS_DIR_ENV_KEY: &str = "CYRUP_PERMISSION_SYSTEM_LOGS_DIR";

/// pi `LOGS_DIR = join(EXTENSION_ROOT, "logs")` (`extension-config.ts:38`). cyrup's analog of pi's
/// `EXTENSION_ROOT` is `<agent_dir>/cyrup-permission-system/` — the same directory
/// `extension::config_path_for` puts `config.json` in — so this is the leaf name joined onto it.
pub const LOGS_DIR_NAME: &str = "logs";

/// pi `getPermissionSystemDebugPath` (`extension-config.ts:52-56`):
/// `join(logsDir, `${EXTENSION_ID}-debug.jsonl`)`.
#[must_use]
pub fn debug_path(logs_dir: &Path) -> PathBuf {
    logs_dir.join(format!("{EXTENSION_ID}-debug.jsonl"))
}

/// pi `getPermissionSystemLogsDir(logsDir?)` (`extension-config.ts:48-51`):
/// `logsDir || overrideDir || LOGS_DIR`. As with
/// [`crate::ext_config::ExtensionConfig::resolve_config_path`], this crate's call site always
/// supplies the computed default (the analog of pi's `LOGS_DIR`) rather than an optional explicit
/// override, so the precedence collapses to `env (trimmed, non-empty) || default_dir`.
///
/// Resolved per WRITE, not once at construction — pi re-reads `process.env` inside `getDebugPath`
/// on every `writeLine` (`logging.ts:47`), so an env change mid-session redirects the trail.
#[must_use]
pub fn resolve_logs_dir(default_dir: &Path) -> PathBuf {
    std::env::var(LOGS_DIR_ENV_KEY)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_dir.to_path_buf())
}

/// pi `ensurePermissionSystemLogsDirectory` (`extension-config.ts:163-171`): `mkdir -p` the log
/// directory, returning `None` on success and the human-readable failure message otherwise (pi
/// returns `undefined` / the string — the message text is reproduced verbatim modulo the
/// `permission-system` product word).
#[must_use]
pub fn ensure_logs_directory(logs_dir: &Path) -> Option<String> {
    match std::fs::create_dir_all(logs_dir) {
        Ok(()) => None,
        Err(err) => Some(format!(
            "Failed to create permission-system log directory '{}': {err}",
            logs_dir.display()
        )),
    }
}

/// pi `safeJsonStringify` (`logging.ts:10-33`): serialize `value` to one line, never throwing.
///
/// pi's replacer exists to survive three JS-only hazards its `details` records can carry — `Error`
/// instances (serialized to `{name, message, stack}`), `bigint` (stringified) and reference CYCLES
/// (`"[Circular]"`). None can occur here: a `serde_json::Value` is a finite acyclic tree with no
/// `Error`/`bigint` variant, so the port is `Value::to_string` plus pi's "never throw" contract —
/// which for `serde_json` means the `Err` arm (unreachable for a plain `Value`, but handled rather
/// than unwrapped per the crate's no-panic policy) maps to `None`, exactly like pi's
/// `JSON.stringify` returning `undefined`.
#[must_use]
pub fn safe_json_stringify(value: &Value) -> Option<String> {
    serde_json::to_string(value).ok()
}

/// pi `PermissionSystemLogger` (`logging.ts:35-39`), built by `createPermissionSystemLogger`
/// (`logging.ts:47-108`).
///
/// Holds the SAME `Arc<Mutex<ExtensionConfig>>` the extension re-reads on every
/// `session_start` / `resources_discover` reload, so flipping `"debug": true` in `config.json` and
/// starting a new session arms the trail — the analog of pi reading the module-scope
/// `extensionConfig` binding through its `getConfig` closure (`index.ts:148-150`).
pub struct PermissionSystemLogger {
    /// pi's `options.getConfig` closure (`logging.ts:42`) — read on EVERY `debug`/`review` so a
    /// mid-session `refreshExtensionConfig` takes effect immediately.
    config: SharedExtensionConfig,
    /// The `LOGS_DIR` analog: `<agent_dir>/cyrup-permission-system/logs`, overridable per write via
    /// [`LOGS_DIR_ENV_KEY`].
    default_logs_dir: PathBuf,
    /// `[CYRUP-DELTA] 1` — the `writeQueue` analog. Held across `open` + `write_all` so two
    /// concurrent decisions can never interleave a half-written line.
    write_lock: Mutex<()>,
}

impl PermissionSystemLogger {
    /// pi `createPermissionSystemLogger({ getConfig })` (`index.ts:148-150`).
    #[must_use]
    pub fn new(config: SharedExtensionConfig, default_logs_dir: PathBuf) -> Self {
        PermissionSystemLogger { config, default_logs_dir, write_lock: Mutex::new(()) }
    }

    /// pi `logger.debug` (`logging.ts:88-94`): the diagnostic stream. Gated on `config.debug` — a
    /// disabled logger returns `None` having touched no filesystem at all (not even the `mkdir`).
    pub fn debug(&self, event: &str, details: &Value) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        self.write_line("debug", event, details)
    }

    /// pi `logger.review` (v0.8.0 `logging.ts:98-100`): the SECURITY-relevant stream — every
    /// decision the gate reaches — written UNCONDITIONALLY.
    ///
    /// At v0.7.1 this stream carried the same `if (!options.getConfig().debug) return undefined;`
    /// early return `debug` still carries (v0.7.1 `logging.ts:97-100`); v0.8.0 deletes those four
    /// lines, leaving `review` a bare `return writeLine("review", event, details);`. An audit trail
    /// that is off unless an operator first opted into diagnostics is not an audit trail: the
    /// entries that matter most (`permission_request.blocked`, `*.approval_persisted`) are exactly
    /// the ones nobody thinks to enable before the incident.
    ///
    /// SCOPE, stated precisely so this doc does not overstate what un-gating delivers: upstream
    /// v0.8.0 `index.ts` has 18 `writeReviewEntry` call sites; cyrup currently has 6, all in
    /// `extension.rs`. The whole FORWARDING half of the trail is unported — `forwarding.rs` holds
    /// no logger reference at all — so the `forwarded_permission.*` entries an operator wants when
    /// a child's ask times out or auto-approves are still absent, gate or no gate. Un-gating makes
    /// the 6 sites we do have reachable; it does not by itself produce v0.8.0's audit trail.
    /// Tracked as G131b in `docs/gap-analysis/PARITY-GAPS.md`.
    ///
    /// `debug` — the diagnostic/lifecycle stream — is deliberately still gated; the `debug` flag
    /// keeps its upstream meaning and this is not a rename of it.
    pub fn review(&self, event: &str, details: &Value) -> Option<String> {
        self.write_line("review", event, details)
    }

    /// pi `logger.flush` (`logging.ts:106`). `[CYRUP-DELTA] 2` — a no-op: the write is already on
    /// the fd when `debug`/`review` returns. Kept so the pi call sites stay 1:1 recognizable.
    pub fn flush(&self) {}

    /// `options.getConfig().debug` (v0.8.0 `logging.ts:91`) — read by the `debug` stream ONLY;
    /// `review` is unconditional since v0.8.0.
    fn enabled(&self) -> bool {
        self.config.lock().map_or_else(|e| e.into_inner().debug, |c| c.debug)
    }

    /// pi `writeLine` (`logging.ts:64-86`): resolve the path, `mkdir -p` the log dir (returning its
    /// failure message unwritten, pi `:67-70`), serialize
    /// `{timestamp, extension, stream, event, ...details}` and append it plus a newline.
    fn write_line(&self, stream: &str, event: &str, details: &Value) -> Option<String> {
        let logs_dir = resolve_logs_dir(&self.default_logs_dir);
        let path = debug_path(&logs_dir);
        if let Some(directory_error) = ensure_logs_directory(&logs_dir) {
            return Some(directory_error);
        }

        // pi's object-spread order (`logging.ts:71-77`): the four fixed keys first, then `details`
        // — so a `details` key of the same name overwrites, exactly as `...details` does.
        let mut record = Map::new();
        record.insert("timestamp".to_string(), Value::String(iso_timestamp()));
        record.insert("extension".to_string(), Value::String(EXTENSION_ID.to_string()));
        record.insert("stream".to_string(), Value::String(stream.to_string()));
        record.insert("event".to_string(), Value::String(event.to_string()));
        if let Value::Object(map) = details {
            for (key, value) in map {
                record.insert(key.clone(), value.clone());
            }
        }

        let Some(line) = safe_json_stringify(&Value::Object(record)) else {
            // pi `:78-80` — verbatim.
            return Some(format!(
                "Failed to write permission-system {stream} entry '{}': event could not be serialized.",
                path.display()
            ));
        };

        // `[CYRUP-DELTA] 1` + `3`: serialized under the write lock, and the io error is REPORTED
        // rather than swallowed (see the module header). A poisoned lock still writes — a panicking
        // writer elsewhere must not silently disable the audit trail.
        let _guard = self.write_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let append = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(format!("{line}\n").as_bytes()));
        match append {
            Ok(()) => None,
            Err(err) => Some(format!(
                "Failed to write permission-system {stream} entry '{}': {err}",
                path.display()
            )),
        }
    }
}

/// pi `new Date().toISOString()` (`logging.ts:72`) — UTC, millisecond precision, `Z` suffix.
/// Hand-formatted (rather than `time`'s `Rfc3339`, which emits variable sub-second digits) so the
/// JSONL timestamps are byte-shaped like pi's.
fn iso_timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond()
    )
}

/// pi `createSensitiveLogMetadata` (`index.ts:682-692`): a prompt / denial reason is NOT written to
/// the trail in the clear alone — it is accompanied by `{present, length, sha256}` so two entries
/// can be correlated (and a redaction pass can drop the plaintext) without re-reading the secret.
/// `None` for an absent value, matching pi's `null`.
#[must_use]
pub fn sensitive_log_metadata(value: Option<&str>) -> Value {
    use sha2::{Digest, Sha256};

    let Some(value) = value else {
        return Value::Null;
    };
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let hex = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>();
    serde_json::json!({ "present": true, "length": value.len(), "sha256": hex })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::sync::{Arc, Mutex as StdMutex};

    use super::*;
    use crate::ext_config::ExtensionConfig;

    /// [`resolve_logs_dir`] re-reads [`LOGS_DIR_ENV_KEY`] on EVERY write, and this crate runs its
    /// unit tests in one process, so `logs_dir_env_var_overrides_the_default`'s process-global
    /// `set_var` is visible to any sibling test that happens to write inside that window — it
    /// silently redirects the sibling's trail into the override directory. Every test that touches
    /// the trail therefore takes the SAME lock `ext_config` uses for its env-dependent tests
    /// (`ext_config.rs:234-243`). Without this the module is flaky at HEAD independently of any
    /// behaviour change, which makes a revert proof unreadable.
    fn trail_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::ext_config::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn logger_with(debug: bool, dir: &Path) -> PermissionSystemLogger {
        let config = ExtensionConfig { debug, ..ExtensionConfig::default() };
        PermissionSystemLogger::new(Arc::new(StdMutex::new(config)), dir.join(LOGS_DIR_NAME))
    }

    fn read_lines(dir: &Path) -> Vec<Value> {
        let path = debug_path(&dir.join(LOGS_DIR_NAME));
        let text = std::fs::read_to_string(path).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).collect()
    }

    // Regression test for pi `logging.ts:64-86` + `:96-102`: a `review` entry under
    // `"debug": true` must APPEND one JSON line shaped `{timestamp, extension, stream, event,
    // ...details}` to `<logsDir>/cyrup-permission-system-debug.jsonl`. Pre-fix the whole module did
    // not exist, so nothing was ever written.
    #[test]
    fn review_entry_appends_a_shaped_jsonl_line() {
        let _env = trail_lock();
        let dir = tempfile::tempdir().unwrap();
        let logger = logger_with(true, dir.path());

        assert_eq!(logger.review("permission_request.blocked", &serde_json::json!({"toolName": "bash"})), None);
        logger.flush();

        let lines = read_lines(dir.path());
        assert_eq!(lines.len(), 1, "exactly one line must be appended");
        let entry = &lines[0];
        assert_eq!(entry["extension"], Value::String(EXTENSION_ID.to_string()));
        assert_eq!(entry["stream"], Value::String("review".to_string()));
        assert_eq!(entry["event"], Value::String("permission_request.blocked".to_string()));
        assert_eq!(entry["toolName"], Value::String("bash".to_string()));
        let ts = entry["timestamp"].as_str().unwrap();
        assert!(ts.ends_with('Z') && ts.len() == 24, "pi `toISOString()` shape, got {ts}");
    }

    // v0.8.0 `logging.ts:98-100`: `review` is a bare `writeLine` — the four-line
    // `if (!options.getConfig().debug) return undefined;` guard v0.7.1 carried at `:97-100` is
    // gone. With `"debug": false` (what `default_config_content()` materializes) the trail must
    // still be written.
    #[test]
    fn review_is_written_with_debug_disabled() {
        let _env = trail_lock();
        let dir = tempfile::tempdir().unwrap();
        let logger = logger_with(false, dir.path());

        assert_eq!(logger.review("permission_request.blocked", &serde_json::json!({"n": 1})), None);

        let lines = read_lines(dir.path());
        assert_eq!(lines.len(), 1, "the security-review stream is not gated on `debug`");
        assert_eq!(lines[0]["stream"], Value::String("review".to_string()));
        assert_eq!(lines[0]["event"], Value::String("permission_request.blocked".to_string()));
    }

    // MIRROR for the above: v0.8.0 `logging.ts:90-93` keeps the guard on the DIAGNOSTIC stream, so
    // un-gating `review` must not un-gate `debug`. `debug` alone under `"debug": false` must touch
    // no filesystem at all — not even the `mkdir`.
    #[test]
    fn debug_stream_stays_gated_and_creates_no_directory() {
        let _env = trail_lock();
        let dir = tempfile::tempdir().unwrap();
        let logger = logger_with(false, dir.path());

        assert_eq!(logger.debug("config.loaded", &serde_json::json!({})), None);

        assert!(
            !dir.path().join(LOGS_DIR_NAME).exists(),
            "a `debug`-disabled diagnostic stream must touch no filesystem"
        );
    }

    // pi `logging.ts:50-62`: successive entries APPEND rather than truncate, and both streams share
    // the one file.
    #[test]
    fn entries_append_and_share_one_file_across_streams() {
        let _env = trail_lock();
        let dir = tempfile::tempdir().unwrap();
        let logger = logger_with(true, dir.path());

        logger.review("a", &serde_json::json!({"n": 1}));
        logger.debug("b", &serde_json::json!({"n": 2}));
        logger.review("c", &serde_json::json!({"n": 3}));

        let lines = read_lines(dir.path());
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["stream"], Value::String("review".to_string()));
        assert_eq!(lines[1]["stream"], Value::String("debug".to_string()));
        assert_eq!(lines[2]["event"], Value::String("c".to_string()));
    }

    // pi `getPermissionSystemLogsDir` (`extension-config.ts:48-51`): the env var overrides the
    // computed default directory.
    #[test]
    fn logs_dir_env_var_overrides_the_default() {
        let _guard = crate::ext_config::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let overridden = dir.path().join("elsewhere");
        let logger = logger_with(true, dir.path());

        // SAFETY: serialized by `env_lock` so no other test observes a partial mutation; restored
        // before the assertions.
        unsafe {
            std::env::set_var(LOGS_DIR_ENV_KEY, overridden.display().to_string());
        }
        let result = logger.review("permission_request.blocked", &serde_json::json!({}));
        unsafe {
            std::env::remove_var(LOGS_DIR_ENV_KEY);
        }

        assert_eq!(result, None);
        assert!(debug_path(&overridden).exists(), "the override dir must receive the trail");
        assert!(!dir.path().join(LOGS_DIR_NAME).exists(), "the default dir must be untouched");
    }

    // pi `createSensitiveLogMetadata` (`index.ts:682-692`).
    #[test]
    fn sensitive_metadata_is_null_for_absent_and_hashed_otherwise() {
        assert_eq!(sensitive_log_metadata(None), Value::Null);
        let meta = sensitive_log_metadata(Some("abc"));
        assert_eq!(meta["present"], Value::Bool(true));
        assert_eq!(meta["length"], serde_json::json!(3));
        assert_eq!(
            meta["sha256"],
            Value::String(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string()
            )
        );
    }

    // pi `logging.ts:67-70`: an unusable log directory returns the `ensureLogsDirectory` message
    // instead of writing (and never panics).
    #[test]
    fn unusable_logs_directory_returns_a_warning() {
        let _env = trail_lock();
        let dir = tempfile::tempdir().unwrap();
        // A regular FILE where the logs directory should be: `mkdir -p` cannot succeed.
        std::fs::write(dir.path().join(LOGS_DIR_NAME), "not a directory").unwrap();
        let logger = logger_with(true, dir.path());

        let warning = logger
            .review("permission_request.blocked", &serde_json::json!({}))
            .expect("an un-creatable log directory must be reported");
        assert!(
            warning.starts_with("Failed to create permission-system log directory"),
            "unexpected warning: {warning}"
        );
    }
}
