//! SUBA-074 stage 2 — `pi-subagents/src/runs/shared/external-cli-preflight.ts` (@v0.64.0): resolve
//! the binary, probe `--version`/`--help`, validate the answers, and cache the result on
//! `(binary path, mtime, spec)` with typed invalidation.
//!
//! This is what makes an adapter's argv SAFE to send: the `claude-code` launch hands the foreign
//! process `--permission-mode plan --tools "" --strict-mcp-config`, and every one of those flags is
//! load-bearing for the sandbox. A binary that does not document them is not the binary the adapter
//! was written against, so it is refused before the prompt is delivered rather than after.
//!
//! [CYRUP-DELTA] upstream's `validate` is a per-adapter CLOSURE (`claude-code-adapter.ts:120-125`).
//! Here the validation is DATA on the spec — [`PreflightSpec::required_help`] plus an optional
//! [`PreflightSpec::version_validator`] function pointer — so the spec is `Clone`, comparable, and
//! usable as its own cache key without a hand-written identity.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// `MAX_PROBE_OUTPUT_BYTES` (`external-cli-preflight.ts:5`).
const MAX_PROBE_OUTPUT_BYTES: usize = 256 * 1024;
/// `MAX_PROBE_TIMEOUT_MS` (`:6`) — the ceiling a spec may only narrow.
const MAX_PROBE_TIMEOUT_MS: u64 = 5_000;
/// `MAX_CACHE_ENTRIES` (`:7`).
const MAX_CACHE_ENTRIES: usize = 64;

/// `ExternalCliPreflightInvalidationReason` (`:9`) — why a cached probe is being dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidationReason {
    /// The launch itself failed.
    Launch,
    /// The failure looks like an authentication problem.
    Auth,
    /// The failure looks like a permission problem.
    Permission,
    /// The adapter's stream parser rejected the output.
    Parser,
}

/// `classifyInvalidation(error)` (`external-cli-runner.ts:138-142`).
///
/// [CYRUP-DELTA] upstream's two case-insensitive regexes are expressed as lowercase substring
/// searches; this crate carries no regex dependency and the patterns are plain alternations with no
/// anchors or classes, so the two are equivalent.
#[must_use]
pub fn classify_invalidation(error: &str) -> InvalidationReason {
    let lowered = error.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| lowered.contains(needle));
    if has(&[
        "auth",
        "unauthorized",
        "unauthorised",
        "credential",
        "login",
    ]) {
        return InvalidationReason::Auth;
    }
    // `read.?only` — one optional character between the two words.
    let read_only = ["read-only", "read only", "readonly"]
        .iter()
        .any(|needle| lowered.contains(needle));
    if read_only || has(&["permission", "forbidden", "denied"]) {
        return InvalidationReason::Permission;
    }
    InvalidationReason::Launch
}

/// An adapter's `--version` check — the function half of upstream's per-adapter `validate` closure
/// (`claude-code-adapter.ts:120-125`), kept as a plain `fn` pointer so a [`PreflightSpec`] stays
/// `Clone` and carries no captured state.
pub type VersionValidator = fn(&str) -> Result<(), String>;

/// `ExternalCliPreflightSpec` (`:11-19`).
///
/// Deliberately NOT `PartialEq`: [`Self::version_validator`] is a function pointer, whose addresses
/// carry no meaningful identity. The spec's identity for caching is [`Self::key`], which is built
/// from the DATA fields — including `required_help`, the data half of upstream's validate closure.
#[derive(Debug, Clone)]
pub struct PreflightSpec {
    /// The adapter id this spec belongs to; part of the cache key.
    pub id: String,
    /// Argv for the version probe.
    pub version_args: Vec<String>,
    /// Argv for the help probe.
    pub help_args: Vec<String>,
    /// `None` uses the code-owned [`MAX_PROBE_TIMEOUT_MS`] ceiling.
    pub probe_timeout_ms: Option<u64>,
    /// Every string that MUST appear in the help output.
    pub required_help: Vec<String>,
    /// An adapter-specific version-string check.
    pub version_validator: Option<VersionValidator>,
}

impl PreflightSpec {
    /// `specKey(spec)` (`:76-78`) — the identity a cache entry is keyed on. The validator is part
    /// of it through `required_help`, which is the data half of upstream's closure.
    fn key(&self) -> String {
        format!(
            "{}|{:?}|{:?}|{:?}|{:?}",
            self.id, self.version_args, self.help_args, self.probe_timeout_ms, self.required_help
        )
    }

    /// `narrowPositiveInteger(spec.probeTimeoutMs, MAX_PROBE_TIMEOUT_MS, …)` (`:70-74`).
    ///
    /// # Errors
    ///
    /// Upstream's message when the spec tries to WIDEN the code-owned ceiling.
    fn probe_timeout(&self) -> Result<u64, String> {
        match self.probe_timeout_ms {
            None => Ok(MAX_PROBE_TIMEOUT_MS),
            Some(value) if value > 0 && value <= MAX_PROBE_TIMEOUT_MS => Ok(value),
            Some(_) => Err(format!(
                "probeTimeoutMs may only narrow the code-owned ceiling of {MAX_PROBE_TIMEOUT_MS}."
            )),
        }
    }
}

/// `ExternalCliPreflightResult` (`:21-28`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightResult {
    /// The resolved, real path of the binary that will actually be spawned.
    pub binary_path: PathBuf,
    /// Its mtime in epoch milliseconds — half the cache key, so an upgraded CLI re-probes.
    pub binary_mtime_ms: i64,
    /// Trimmed `--version` output.
    pub version: String,
    /// Trimmed `--help` output.
    pub help: String,
    /// Whether the probes were served from cache.
    pub cache_hit: bool,
}

#[derive(Debug, Clone)]
struct CachedPreflight {
    binary_path: PathBuf,
    binary_mtime_ms: i64,
    version: String,
    help: String,
}

/// The probe cache. Two maps, exactly as upstream (`:32-33`): a lookup keyed on
/// `(binary, mtime, spec)` and the entries themselves keyed on `(binary, version, mtime, spec)`, so
/// an entry can be evicted by either identity.
static CACHE: LazyLock<Mutex<PreflightCache>> =
    LazyLock::new(|| Mutex::new(PreflightCache::default()));

#[derive(Debug, Default)]
struct PreflightCache {
    entries: Vec<(String, CachedPreflight)>,
    lookup: BTreeMap<String, String>,
}

/// `resolveBinary(command, env)` (`:35-53`).
///
/// `path_var` is the child's OWN `PATH` — the allowlisted projection's, never necessarily this
/// process's — because the binary that is probed must be the binary that is spawned.
///
/// # Errors
///
/// Upstream's `External CLI binary '<command>' was not found on PATH.`, and the `accessSync`
/// failure for an explicit path that is not executable.
pub fn resolve_binary(command: &str, path_var: Option<&str>) -> Result<PathBuf, String> {
    let explicit = Path::new(command);
    if explicit.is_absolute() || command.contains(std::path::MAIN_SEPARATOR) {
        let resolved = std::path::absolute(explicit)
            .map_err(|error| format!("External CLI binary '{command}' is unusable: {error}"))?;
        if !is_executable(&resolved) {
            return Err(format!(
                "External CLI binary '{command}' is not an executable file."
            ));
        }
        return Ok(resolved);
    }
    for directory in std::env::split_paths(path_var.unwrap_or_default()) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let candidate = directory.join(command);
        if is_executable(&candidate) {
            // `fs.realpathSync(candidate)` — the cache key must name the file itself, so two
            // symlinks to one binary share an entry.
            return Ok(candidate.canonicalize().unwrap_or(candidate));
        }
    }
    Err(format!(
        "External CLI binary '{command}' was not found on PATH."
    ))
}

/// `fs.accessSync(path, X_OK)` — a regular file with an execute bit.
fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `probeWithTimeout(binaryPath, args, env, label, timeoutMs, cwd)` (`:55-68`).
///
/// # Errors
///
/// Upstream's two refusals: a spawn/timeout failure, and a non-zero exit with the probe's own
/// stderr (or stdout) quoted.
async fn probe_with_timeout(
    binary_path: &Path,
    args: &[String],
    env: Option<&BTreeMap<String, String>>,
    label: &str,
    timeout_ms: u64,
    cwd: &Path,
) -> Result<String, String> {
    let mut command = tokio::process::Command::new(binary_path);
    command.args(args);
    command.current_dir(cwd);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    if let Some(env) = env {
        command.env_clear();
        command.envs(env);
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("External CLI {label} preflight failed: {error}"))?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let drain_stdout = stdout.map(|pipe| tokio::spawn(drain(pipe)));
    let drain_stderr = stderr.map(|pipe| tokio::spawn(drain(pipe)));
    // `biased` for the same reason `exec::acceptance::model::verify::run` uses it: a probe that
    // already exited must never be reported as a timeout because the two arms were both ready.
    let waited = tokio::select! {
        biased;
        status = child.wait() => Some(status),
        () = tokio::time::sleep_until(deadline) => None,
    };
    let Some(status) = waited else {
        // `killSignal: "SIGKILL"` (`:60`) — the probe gets no grace period.
        crate::spawn::signal::send_sigkill(&mut child);
        let _ = child.wait().await;
        return Err(format!(
            "External CLI {label} preflight failed: timed out after {timeout_ms}ms"
        ));
    };
    let status =
        status.map_err(|error| format!("External CLI {label} preflight failed: {error}"))?;
    let out = match drain_stdout {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    };
    let err = match drain_stderr {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    };
    if !status.success() {
        let code = status
            .code()
            .map_or_else(|| "null".to_string(), |c| c.to_string());
        let detail = if err.trim().is_empty() { &out } else { &err };
        return Err(format!(
            "External CLI {label} preflight exited with code {code}: {}",
            detail.trim()
        ));
    }
    Ok(out.trim().to_string())
}

/// Read a pipe to at most [`MAX_PROBE_OUTPUT_BYTES`] (upstream's `maxBuffer`, `:61`).
async fn drain<R: tokio::io::AsyncRead + Unpin + Send + 'static>(mut pipe: R) -> String {
    use tokio::io::AsyncReadExt;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    while let Ok(read) = pipe.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        let room = MAX_PROBE_OUTPUT_BYTES.saturating_sub(buffer.len());
        if room == 0 {
            break;
        }
        buffer.extend_from_slice(chunk.get(..read.min(room)).unwrap_or_default());
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

/// `preflightExternalCli(command, spec, env, cwd)` (`:80-109`).
///
/// # Errors
///
/// A binary that cannot be resolved, a probe that fails or times out, a spec that tries to widen
/// the probe timeout, or a validation failure from [`validate`].
pub async fn preflight_external_cli(
    command: &str,
    spec: &PreflightSpec,
    env: Option<&BTreeMap<String, String>>,
    cwd: &Path,
) -> Result<PreflightResult, String> {
    let path_var = match env {
        Some(env) => env.get("PATH").cloned(),
        None => std::env::var("PATH").ok(),
    };
    let binary_path = resolve_binary(command, path_var.as_deref())?;
    let binary_mtime_ms = std::fs::metadata(&binary_path)
        .and_then(|metadata| metadata.modified())
        .map(crate::time::epoch_millis)
        .unwrap_or_default();
    let spec_key = spec.key();
    let lookup_key = format!("{}|{binary_mtime_ms}|{spec_key}", binary_path.display());
    let probe_timeout_ms = spec.probe_timeout()?;

    let cached = {
        let Ok(cache) = CACHE.lock() else {
            return Err("External CLI preflight cache is poisoned.".to_string());
        };
        cache.lookup.get(&lookup_key).and_then(|entry_key| {
            cache
                .entries
                .iter()
                .find(|(key, _)| key == entry_key)
                .map(|(_, entry)| entry.clone())
        })
    };

    let base = match cached.clone() {
        Some(base) => base,
        None => CachedPreflight {
            binary_path: binary_path.clone(),
            binary_mtime_ms,
            version: probe_with_timeout(
                &binary_path,
                &spec.version_args,
                env,
                "version",
                probe_timeout_ms,
                cwd,
            )
            .await?,
            help: probe_with_timeout(
                &binary_path,
                &spec.help_args,
                env,
                "help",
                probe_timeout_ms,
                cwd,
            )
            .await?,
        },
    };

    let result = PreflightResult {
        binary_path: base.binary_path.clone(),
        binary_mtime_ms: base.binary_mtime_ms,
        version: base.version.clone(),
        help: base.help.clone(),
        cache_hit: cached.is_some(),
    };
    // Upstream validates on EVERY call, cache hit included (`:97`) — a cached probe still has to
    // satisfy the current build's expectations.
    validate(spec, &result)?;

    if cached.is_none() {
        let entry_key = format!(
            "{}|{}|{binary_mtime_ms}|{spec_key}",
            binary_path.display(),
            base.version
        );
        if let Ok(mut cache) = CACHE.lock() {
            cache.entries.retain(|(key, _)| key != &entry_key);
            cache.entries.push((entry_key.clone(), base));
            cache.lookup.insert(lookup_key, entry_key);
            while cache.entries.len() > MAX_CACHE_ENTRIES {
                let evicted = cache.entries.remove(0).0;
                cache.lookup.retain(|_, value| value != &evicted);
            }
        }
    }
    Ok(result)
}

/// The data half of upstream's per-adapter `validate` closure (`claude-code-adapter.ts:120-125`).
///
/// # Errors
///
/// The adapter's version refusal, or `<id> help does not document required option "<flag>".` for
/// the first required string the help output does not carry.
fn validate(spec: &PreflightSpec, result: &PreflightResult) -> Result<(), String> {
    if let Some(check) = spec.version_validator {
        check(&result.version)?;
    }
    for required in &spec.required_help {
        if !result.help.contains(required.as_str()) {
            return Err(format!(
                "{} help does not document required option {}.",
                help_label(&spec.id),
                serde_json::Value::String(required.clone())
            ));
        }
    }
    Ok(())
}

/// The human name an adapter uses in its own refusals — upstream writes them inline per adapter
/// (`Claude Code help does not document …`).
fn help_label(adapter_id: &str) -> &'static str {
    match adapter_id {
        "claude-code" | "claude-code-writer" => "Claude Code",
        "codex-exec" | "codex-exec-writer" => "Codex",
        "cursor-agent" | "cursor-agent-writer" => "Cursor Agent",
        _ => "External CLI",
    }
}

/// `invalidateExternalCliPreflight(command, spec, reason)` (`:111-117`).
///
/// Drops every entry whose binary is this command (by full path or by basename) or whose key
/// carries this spec, then prunes any lookup that now points at nothing. The `reason` is recorded
/// by upstream's signature but not used to select entries — it is a typed audit input, and it is
/// carried here for the same reason.
pub fn invalidate_external_cli_preflight(
    command: &str,
    spec: &PreflightSpec,
    _reason: InvalidationReason,
) {
    let spec_key = spec.key();
    let Ok(mut cache) = CACHE.lock() else {
        return;
    };
    cache.entries.retain(|(key, entry)| {
        let path = entry.binary_path.display().to_string();
        let matches_command = path == command
            || entry
                .binary_path
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new(command));
        !(matches_command || key.contains(&spec_key))
    });
    let live: Vec<String> = cache.entries.iter().map(|(key, _)| key.clone()).collect();
    cache.lookup.retain(|_, value| live.contains(value));
}

/// `clearExternalCliPreflightCacheForTests()` (`:119-122`).
#[cfg(test)]
pub(crate) fn clear_preflight_cache_for_tests() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.entries.clear();
        cache.lookup.clear();
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn spec(required_help: &[&str]) -> PreflightSpec {
        PreflightSpec {
            id: "claude-code".to_string(),
            version_args: vec!["--version".to_string()],
            help_args: vec!["--help".to_string()],
            probe_timeout_ms: Some(4_000),
            required_help: required_help.iter().map(|s| (*s).to_string()).collect(),
            version_validator: None,
        }
    }

    /// The PATH search only accepts an EXECUTABLE regular file, and reports upstream's message when
    /// nothing matches (`:35-53`).
    #[test]
    fn the_binary_is_resolved_off_the_childs_own_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("not-exec"), "x").unwrap();
        let script = write_script(dir.path(), "fake-cli", "#!/bin/sh\nexit 0\n");
        let path_var = dir.path().display().to_string();
        assert_eq!(
            resolve_binary("fake-cli", Some(&path_var)).unwrap(),
            script.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_binary("missing-cli", Some(&path_var)).unwrap_err(),
            "External CLI binary 'missing-cli' was not found on PATH."
        );
        #[cfg(unix)]
        assert_eq!(
            resolve_binary("not-exec", Some(&path_var)).unwrap_err(),
            "External CLI binary 'not-exec' was not found on PATH."
        );
        // An explicit path is resolved directly, and a non-executable one is refused by name.
        assert_eq!(
            resolve_binary(&script.display().to_string(), None).unwrap(),
            script
        );
    }

    /// A probe that exits non-zero, and one that hangs, both refuse the launch (`:65-66`, `:60`).
    #[tokio::test]
    async fn a_failing_or_hanging_probe_refuses_the_launch() {
        let dir = tempfile::tempdir().unwrap();
        let failing = write_script(dir.path(), "bad", "#!/bin/sh\necho 'boom' >&2\nexit 3\n");
        let error = probe_with_timeout(&failing, &[], None, "version", 4_000, dir.path())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "External CLI version preflight exited with code 3: boom"
        );

        let hanging = write_script(dir.path(), "hang", "#!/bin/sh\nsleep 30\n");
        let error = probe_with_timeout(&hanging, &[], None, "help", 150, dir.path())
            .await
            .unwrap_err();
        assert!(
            error.starts_with("External CLI help preflight failed: timed out"),
            "{error}"
        );
    }

    /// The whole preflight: probe, validate the required help strings, then serve the SECOND call
    /// from cache — and drop that cache when the run reports a failure (`:80-117`).
    #[tokio::test]
    async fn a_probe_is_validated_cached_and_invalidated() {
        clear_preflight_cache_for_tests();
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "probe-cli",
            "#!/bin/sh\ncase \"$1\" in\n--version) echo '1.2.3 (Fake CLI)';;\n--help) echo 'usage: --permission-mode --tools';;\nesac\n",
        );

        let ok = spec(&["--permission-mode", "--tools"]);
        let first = preflight_external_cli(&script.display().to_string(), &ok, None, dir.path())
            .await
            .unwrap();
        assert!(!first.cache_hit);
        assert_eq!(first.version, "1.2.3 (Fake CLI)");

        let second = preflight_external_cli(&script.display().to_string(), &ok, None, dir.path())
            .await
            .unwrap();
        assert!(
            second.cache_hit,
            "the same (binary, mtime, spec) re-probes nothing"
        );

        invalidate_external_cli_preflight(
            &script.display().to_string(),
            &ok,
            InvalidationReason::Auth,
        );
        let third = preflight_external_cli(&script.display().to_string(), &ok, None, dir.path())
            .await
            .unwrap();
        assert!(!third.cache_hit, "an invalidated entry is re-probed");

        // A missing required option refuses the launch, quoting the option.
        let strict = spec(&["--permission-mode", "--strict-mcp-config"]);
        assert_eq!(
            preflight_external_cli(&script.display().to_string(), &strict, None, dir.path())
                .await
                .unwrap_err(),
            "Claude Code help does not document required option \"--strict-mcp-config\"."
        );
        clear_preflight_cache_for_tests();
    }

    /// The probe timeout is a code-owned ceiling a spec may only narrow (`:70-74`).
    #[test]
    fn the_probe_timeout_may_only_be_narrowed() {
        let mut widened = spec(&[]);
        widened.probe_timeout_ms = Some(MAX_PROBE_TIMEOUT_MS + 1);
        assert_eq!(
            widened.probe_timeout().unwrap_err(),
            format!(
                "probeTimeoutMs may only narrow the code-owned ceiling of {MAX_PROBE_TIMEOUT_MS}."
            )
        );
        let mut default = spec(&[]);
        default.probe_timeout_ms = None;
        assert_eq!(default.probe_timeout().unwrap(), MAX_PROBE_TIMEOUT_MS);
    }

    /// `classifyInvalidation` (`external-cli-runner.ts:138-142`) — auth wins over permission, and
    /// anything unrecognised is a launch failure.
    #[test]
    fn an_error_is_classified_for_invalidation() {
        assert_eq!(
            classify_invalidation("Unauthorized: please login"),
            InvalidationReason::Auth
        );
        assert_eq!(
            classify_invalidation("EACCES: permission denied"),
            InvalidationReason::Permission
        );
        assert_eq!(
            classify_invalidation("sandbox is read-only"),
            InvalidationReason::Permission
        );
        assert_eq!(
            classify_invalidation("exited with code 2"),
            InvalidationReason::Launch
        );
    }
}
