//! Artifact quadruple writer + housekeeping sweep — Rust port of pi `shared/artifacts.ts`
//! (T6, remediation §Tier-6 "Artifact quadruple").
//!
//! Every foreground subagent run leaves a small, inspectable on-disk record under a scoped,
//! per-`cwd` artifacts directory: `<runId>_<agent>[_i]_input.md` (the task the child was given),
//! `<runId>_<agent>[_i]_output.md` (its delivered answer), `<runId>_<agent>[_i].jsonl` (the run's
//! observable event stream), and `<runId>_<agent>[_i]_meta.json` (usage/model/exit-code metadata) —
//! matching pi's `getArtifactPaths` (`shared/artifacts.ts:186-197`). The four filenames and the
//! `[^\w.-] -> _` agent-name sanitization are byte-for-byte faithful to pi so an artifact consumer
//! (a human, a follow-up run, a debugging tool) sees the exact same layout.
//!
//! Housekeeping mirrors pi one-for-one: [`cleanup_old_artifacts`] is the 24h-throttled 7-day sweep
//! (`shared/artifacts.ts:230-259`, a `.last-cleanup` marker gates re-scanning to once per day and
//! deletes any artifact older than `max_age_days`), [`cleanup_all_artifact_dirs`] fans that sweep
//! across the temp + per-session artifact roots (`shared/artifacts.ts:261-285`), and
//! [`cleanup_old_chain_dirs`] is pi's separate 24h chain-runs sweep (`shared/settings.ts:197-220`).
//! All housekeeping is best-effort: a file that vanishes or is unreadable mid-scan is skipped, never
//! fatal to the caller (extension startup must not fail on a stale artifact).
//!
//! Directory scoping reuses the SAME `<home>/.cyrup/subagents/<subdir>/<cwd_key>` layout the
//! background async/results roots use ([`crate::background::run_artifact_roots`]) so a project's
//! artifacts, chain-runs, async runs, and results all live together under one per-`cwd` scope —
//! the Rust analog of pi's shared scoped `TEMP_ROOT_DIR`.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cyrup_core::ModelId;

use crate::background::{cwd_key, temp_root_dir};
use crate::exec::SingleResult;
use crate::paths::agent_dir;

/// Project-local artifact root (pi `PROJECT_ARTIFACT_ROOT = ".pi-subagents"`, rebranded).
const PROJECT_ARTIFACT_ROOT: &str = ".cyrup-subagents";
/// The `artifacts` leaf under both the project root and the scoped temp root.
const ARTIFACTS_SUBDIR: &str = "artifacts";
/// The `chain-runs` leaf (pi `CHAIN_RUNS_DIR`'s leaf + `getProjectChainRunsDir`).
const CHAIN_RUNS_SUBDIR: &str = "chain-runs";
/// Per-session artifact leaf under a session directory (pi `getArtifactsDir` session branch).
const SESSION_ARTIFACTS_SUBDIR: &str = "subagent-artifacts";
/// The throttle marker file pi writes at the root of a swept dir (`shared/artifacts.ts:5`).
const CLEANUP_MARKER_FILE: &str = ".last-cleanup";

/// One day, in milliseconds — the sweep throttle window (pi `24 * 60 * 60 * 1000`) and the
/// chain-runs max age (pi `CHAIN_DIR_MAX_AGE_MS`).
const ONE_DAY_MS: u128 = 24 * 60 * 60 * 1000;

/// The default artifact-cleanup horizon (pi `DEFAULT_ARTIFACT_CONFIG.cleanupDays`, `shared/types.ts:1804`).
pub const DEFAULT_CLEANUP_DAYS: u64 = 7;

/// The four artifact paths for one run/agent/index (pi `ArtifactPaths`, `shared/types.ts:1044-1050`).
///
/// `Serialize` (camelCase, matching pi's own field names) because pi carries this bundle onto a
/// result as `SingleResult.artifactPaths` (`shared/types.ts:901`) and spreads it verbatim into a
/// dynamic fan-out's collect records (`runs/shared/dynamic-fanout.ts:286`), where a chain author
/// reads it through `{outputs.<collect.as>}`. Only serialization is derived: nothing reads one of
/// these back off the wire, and pi's fifth field (`transcriptPath`) has no analogue in this port.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPaths {
    /// `<base>_input.md` — the task the child was given.
    pub input_path: PathBuf,
    /// `<base>_output.md` — the child's delivered answer.
    pub output_path: PathBuf,
    /// `<base>.jsonl` — the run's observable NDJSON event stream.
    pub jsonl_path: PathBuf,
    /// `<base>_meta.json` — usage/model/exit-code metadata.
    pub metadata_path: PathBuf,
}

/// Which of the four artifact files to write + the cleanup horizon (pi `ArtifactConfig`,
/// `shared/types.ts:1054-1063`). The [`Default`] impl reproduces pi's `DEFAULT_ARTIFACT_CONFIG`
/// (`shared/types.ts:1796-1805`) exactly: input/output/metadata on, **jsonl off**, 7-day cleanup.
///
/// SUBA-N03: `Serialize`/`Deserialize` (camelCase, matching pi's own `artifactConfig` wire shape)
/// because the async SINGLE path carries this whole config to the detached hop-2 runner on
/// [`crate::background::runner_main::RunnerConfig::artifact_config`] — pi's `spawnRunner({ …,
/// artifactsDir, artifactConfig, … })` (`runs/background/async-execution.ts:966-968` @v0.34.0),
/// read back by its runner as `ctx.artifactConfig?.enabled !== false`
/// (`runs/background/subagent-runner.ts:879-890,1117-1125` @v0.34.0). Every field is `#[serde(default)]`ed
/// through a whole-struct `Default` so an older on-disk config that omits the block still
/// deserializes to pi's own `DEFAULT_ARTIFACT_CONFIG`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArtifactConfig {
    /// Master switch: when `false`, nothing is written at all (pi `enabled`).
    pub enabled: bool,
    /// Write `_input.md` (pi `includeInput`, default `true`).
    pub include_input: bool,
    /// Write `_output.md` (pi `includeOutput`, default `true`).
    pub include_output: bool,
    /// Write `.jsonl` (pi `includeJsonl`, default `false`).
    pub include_jsonl: bool,
    /// Write `_meta.json` (pi `includeMetadata`, default `true`).
    pub include_metadata: bool,
    /// Delete artifacts older than this many days on sweep (pi `cleanupDays`, default `7`).
    pub cleanup_days: u64,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        // pi `DEFAULT_ARTIFACT_CONFIG` (`shared/types.ts:1796-1805`).
        Self {
            enabled: true,
            include_input: true,
            include_output: true,
            include_jsonl: false,
            include_metadata: true,
            cleanup_days: DEFAULT_CLEANUP_DAYS,
        }
    }
}

impl ArtifactConfig {
    /// The config the foreground single-run path writes with (T6): identical to pi's default EXCEPT
    /// that the `.jsonl` event stream is enabled, so every foreground run leaves the full artifact
    /// quadruple (`_input.md`/`_output.md`/`.jsonl`/`_meta.json`) rather than only three files. pi
    /// leaves `includeJsonl` off in its GLOBAL default but writes the same `.jsonl` path verbatim
    /// once a caller enables it (`runs/foreground/execution.ts:1468-1470`); this crate opts the
    /// foreground path into it so the run's observable event stream is always captured alongside its
    /// input/output/metadata.
    #[must_use]
    pub fn foreground() -> Self {
        Self {
            include_jsonl: true,
            ..Self::default()
        }
    }
}

/// `<cwd>/.cyrup-subagents` (pi `getProjectSubagentsDir`, `shared/artifacts.ts:133-135`).
#[must_use]
pub fn project_subagents_dir(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_ARTIFACT_ROOT)
}

/// `<cwd>/.cyrup-subagents/artifacts` (pi `getProjectArtifactsDir`, `shared/artifacts.ts:137-139`).
#[must_use]
pub fn project_artifacts_dir(cwd: &Path) -> PathBuf {
    project_subagents_dir(cwd).join(ARTIFACTS_SUBDIR)
}

/// `<cwd>/.cyrup-subagents/chain-runs` (pi `getProjectChainRunsDir`, `shared/artifacts.ts:141-143`).
#[must_use]
pub fn project_chain_runs_dir(cwd: &Path) -> PathBuf {
    project_subagents_dir(cwd).join(CHAIN_RUNS_SUBDIR)
}

/// pi `TEMP_ARTIFACTS_DIR = path.join(TEMP_ROOT_DIR, "artifacts")` (`shared/types.ts:1866`
/// @v0.43.0), keyed per-`cwd` under the SAME [`crate::background::temp_root_dir`] the async/results
/// dirs use ([`crate::background::run_artifact_roots`]).
///
/// [CYRUP-DELTA] the `<cwd_key>` level is cyrup's; upstream's is flat. See
/// [`crate::background::cwd_key`] for why.
#[must_use]
pub fn temp_artifacts_dir(cwd: &Path) -> PathBuf {
    temp_root_dir().join(ARTIFACTS_SUBDIR).join(cwd_key(cwd))
}

/// pi `CHAIN_RUNS_DIR = path.join(TEMP_ROOT_DIR, "chain-runs")` (`shared/types.ts:1865` @v0.43.0),
/// keyed per-`cwd` alongside [`temp_artifacts_dir`].
#[must_use]
pub fn chain_runs_dir(cwd: &Path) -> PathBuf {
    temp_root_dir().join(CHAIN_RUNS_SUBDIR).join(cwd_key(cwd))
}

/// SUBA-048 — pi `ArtifactDirPreference` (`shared/types.ts`, validated against
/// `ARTIFACT_DIR_PREFERENCES` at `extension/config.ts:9,22-24` @v0.43.0, which THROWS on anything
/// else): where a run's artifact files go.
///
/// Config key `subagents.artifactDir` (`shared/types.ts:1857` @v0.47.1). `project` is upstream's
/// default (`DEFAULT_ARTIFACT_CONFIG.dir`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactDirPreference {
    /// `<cwd>/.cyrup-subagents/artifacts` — pi's default.
    #[default]
    Project,
    /// The active session file's sibling `subagent-artifacts/` directory; falls back to temp when
    /// there is no session file.
    Session,
    /// The OS temp root, keyed per cwd. Chosen specifically to keep generated transcripts, inputs
    /// and outputs OUT of a git working tree.
    Temp,
}

impl ArtifactDirPreference {
    /// pi `extension/config.ts:22-24`'s `ARTIFACT_DIR_PREFERENCES` membership test, with upstream's
    /// exact refusal text.
    ///
    /// # Errors
    ///
    /// `config.artifactDir must be "project", "session", or "temp"` for anything else.
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "project" => Ok(Self::Project),
            "session" => Ok(Self::Session),
            "temp" => Ok(Self::Temp),
            _ => Err(r#"config.artifactDir must be "project", "session", or "temp""#.to_string()),
        }
    }
}

/// Resolve the artifacts directory for a run — pi `getArtifactsDir(sessionFile, projectCwd?,
/// dirPreference = "project")` (`shared/artifacts.ts:160-183` @v0.43.0).
///
/// SUBA-048: this used to take three PATH arguments and no preference, so a `Some(project_cwd)`
/// always won and `"artifactDir": "temp"` / `"session"` were unreachable — every run wrote
/// `<cwd>/.cyrup-subagents/…` into the user's repository no matter what the config said. The three
/// arms below are upstream's, including the fall-throughs: `session` without a session file falls
/// to temp, and `project` without a project cwd falls to the session sibling and only then to temp.
#[must_use]
pub fn resolve_artifacts_dir(
    session_file: Option<&Path>,
    project_cwd: Option<&Path>,
    temp_cwd: &Path,
    preference: ArtifactDirPreference,
) -> PathBuf {
    let session_sibling = || {
        session_file
            .and_then(Path::parent)
            .map(|parent| parent.join(SESSION_ARTIFACTS_SUBDIR))
    };
    match preference {
        ArtifactDirPreference::Session => {
            session_sibling().unwrap_or_else(|| temp_artifacts_dir(temp_cwd))
        }
        ArtifactDirPreference::Temp => temp_artifacts_dir(temp_cwd),
        ArtifactDirPreference::Project => {
            if let Some(project) = project_cwd {
                return project_artifacts_dir(project);
            }
            session_sibling().unwrap_or_else(|| temp_artifacts_dir(temp_cwd))
        }
    }
}

/// Resolve the chain-runs ROOT for a cwd — pi `getChainRunsDir(projectCwd, dirPreference =
/// "project")` (`shared/artifacts.ts:145-158` @v0.43.0).
///
/// SUBA-048 / PARITY-GAPS PB-13: upstream's `project` arm (the default) returns
/// `getProjectChainRunsDir`, i.e. `<cwd>/.pi-subagents/chain-runs`; only `session` and `temp`
/// collapse onto the flat temp `CHAIN_RUNS_DIR`. cyrup previously had no preference parameter here
/// and unconditionally used the temp root, so [`project_chain_runs_dir`] had zero references and a
/// chain run's artifacts were invisible to the project and swept by OS tmp cleanup.
///
/// Note upstream's `session` arm deliberately does NOT mirror [`resolve_artifacts_dir`]'s
/// session-sibling branch — chain runs have no per-session directory upstream either.
#[must_use]
pub fn resolve_chain_runs_dir(project_cwd: &Path, preference: ArtifactDirPreference) -> PathBuf {
    match preference {
        ArtifactDirPreference::Project => project_chain_runs_dir(project_cwd),
        ArtifactDirPreference::Session | ArtifactDirPreference::Temp => chain_runs_dir(project_cwd),
    }
}

/// Replace every character outside pi's `[\w.-]` class with `_` (pi `safeAgent`,
/// `shared/artifacts.ts:188`). `\w` in the pi regex (no `u` flag) is exactly ASCII `[A-Za-z0-9_]`.
fn safe_agent(agent: &str) -> String {
    agent
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The four artifact paths for one run (pi `getArtifactPaths`, `shared/artifacts.ts:186-197`): base is
/// `<runId>_<safeAgent>[_<index>]`, with a per-fan-out `_<index>` suffix only when `index` is set.
#[must_use]
pub fn artifact_paths(
    dir: &Path,
    run_id: &str,
    agent: &str,
    index: Option<usize>,
) -> ArtifactPaths {
    let suffix = index.map_or_else(String::new, |i| format!("_{i}"));
    let base = format!("{run_id}_{}{suffix}", safe_agent(agent));
    ArtifactPaths {
        input_path: dir.join(format!("{base}_input.md")),
        output_path: dir.join(format!("{base}_output.md")),
        jsonl_path: dir.join(format!("{base}.jsonl")),
        metadata_path: dir.join(format!("{base}_meta.json")),
    }
}

/// Create the artifacts dir + every missing parent (pi `ensureArtifactsDir`,
/// `shared/artifacts.ts:199-201`).
///
/// # Errors
/// Propagates the underlying `create_dir_all` error.
pub fn ensure_artifacts_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Write one artifact file's UTF-8 content (pi `writeArtifact`, `shared/artifacts.ts:203-206`).
///
/// # Errors
/// Propagates the underlying write error.
pub fn write_artifact(path: &Path, content: &str) -> io::Result<()> {
    std::fs::write(path, content)
}

/// Write a pretty-printed JSON metadata file (pi `writeMetadata`, `shared/artifacts.ts:221-224`,
/// which uses `JSON.stringify(metadata, null, 2)`).
///
/// # Errors
/// Propagates a serialization or write error.
pub fn write_metadata(path: &Path, metadata: &serde_json::Value) -> io::Result<()> {
    let body = serde_json::to_string_pretty(metadata)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, body)
}

/// Append one newline-terminated line to the `.jsonl` event stream (pi `appendJsonl`,
/// `shared/artifacts.ts:226-228`).
///
/// # Errors
/// Propagates the underlying open/append error.
pub fn append_jsonl(path: &Path, line: &str) -> io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

/// A file's modification time in whole milliseconds since the Unix epoch, or `None` if it cannot be
/// determined (matches pi reading `stat.mtimeMs`).
fn mtime_ms(path: &Path) -> Option<u128> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis())
}

/// `Date.now()` (the crate's one clock, [`crate::time::now_epoch_millis`]) widened to the `u128`
/// milliseconds [`mtime_ms`] reports, so the two are directly comparable. A pre-epoch clock reads
/// as `0`, the same floor the shared helper uses.
fn now_millis_u128() -> u128 {
    u128::try_from(crate::time::now_epoch_millis()).unwrap_or(0)
}

/// 24h-throttled sweep of artifacts older than `max_age_days` in one directory (pi
/// `cleanupOldArtifacts`, `shared/artifacts.ts:230-259`).
///
/// A `.last-cleanup` marker at the dir root gates re-scanning to at most once per 24h: if the marker
/// was touched within the last day the sweep returns immediately (this is what makes the sweep cheap
/// to call on every session start). Otherwise every file whose mtime predates the `max_age_days`
/// cutoff is deleted (the marker itself is always skipped), and the marker is rewritten with the
/// current timestamp. Best-effort throughout — a file that disappears or is unreadable mid-scan is
/// skipped so one bad entry never blocks the rest.
pub fn cleanup_old_artifacts(dir: &Path, max_age_days: u64) {
    // SUBA-059 / pi `if (maxAgeDays <= 0 || !fs.existsSync(dir)) return;`
    // (`shared/artifacts.ts:231` @v0.47.1). `0` DISABLES the sweep — it is upstream's documented
    // opt-out (`shared/types.ts:1858`, "Set cleanupDays to 0 to disable cleanup") and it MUST be
    // checked before the arithmetic below, where `now - 0 * ONE_DAY_MS` is `now` and would delete
    // every artifact rather than none.
    if max_age_days == 0 || !dir.exists() {
        return;
    }

    let marker = dir.join(CLEANUP_MARKER_FILE);
    let now = now_millis_u128();

    // Throttle: skip if the marker was written within the last 24h (pi `now - stat.mtimeMs < 24h`).
    if let Some(marker_mtime) = mtime_ms(&marker)
        && now.saturating_sub(marker_mtime) < ONE_DAY_MS
    {
        return;
    }

    let cutoff = now.saturating_sub(u128::from(max_age_days).saturating_mul(ONE_DAY_MS));

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|n| n.to_str()) == Some(CLEANUP_MARKER_FILE) {
            continue;
        }
        if let Some(file_mtime) = mtime_ms(&path)
            && file_mtime < cutoff
        {
            // Best-effort: a directory or a vanished file is simply skipped.
            let _ = std::fs::remove_file(&path);
        }
    }

    // Rewrite the throttle marker (pi `fs.writeFileSync(markerPath, String(now))`).
    let _ = std::fs::write(&marker, now.to_string());
}

/// Sweep every artifact directory this crate writes to (pi `cleanupAllArtifactDirs`,
/// `shared/artifacts.ts:261-285`): the scoped temp artifacts root for `cwd`, plus each persisted
/// session's sibling `subagent-artifacts` directory under `<home>/.cyrup/sessions`. Best-effort: an
/// unreadable sessions root or session dir is skipped rather than failing startup.
pub fn cleanup_all_artifact_dirs(cwd: &Path, max_age_days: u64) {
    cleanup_old_artifacts(&temp_artifacts_dir(cwd), max_age_days);

    // pi `const sessionsBase = path.join(getAgentDir(), "sessions")`
    // (`shared/artifacts.ts:264` @v0.43.0). This previously read `temp_root_dir().parent()/sessions`,
    // which resolved to `<home>/.cyrup/sessions` — a directory cyrup never writes; the real one is
    // `<agent_dir>/sessions`, so the per-session sweep below swept nothing at all.
    let sessions_base = agent_dir().join("sessions");
    let Ok(entries) = std::fs::read_dir(&sessions_base) else {
        return;
    };
    for entry in entries.flatten() {
        let artifacts_dir = entry.path().join(SESSION_ARTIFACTS_SUBDIR);
        cleanup_old_artifacts(&artifacts_dir, max_age_days);
    }
}

/// Remove chain-run scratch directories older than 24h (pi `cleanupOldChainDirs`,
/// `shared/settings.ts:197-220`). Unlike [`cleanup_old_artifacts`] this is unthrottled and operates
/// on whole subdirectories (each `<chainRunsDir>/<runId>/`), matching pi. Best-effort: a dir that
/// cannot be stat'd or removed is skipped.
pub fn cleanup_old_chain_dirs(cwd: &Path) {
    let dir = chain_runs_dir(cwd);
    if !dir.exists() {
        return;
    }
    let now = now_millis_u128();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        if let Some(dir_mtime) = mtime_ms(&path)
            && now.saturating_sub(dir_mtime) > ONE_DAY_MS
        {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// The four content payloads for one run's artifact quadruple (T6), passed as a bundle to
/// [`write_run_artifacts`] so it stays under clippy's argument-count ceiling.
#[derive(Clone, Copy, Debug)]
pub struct RunArtifactContent<'a> {
    /// The `_input.md` body — the task the child was given (pi writes `# Task for <agent>\n\n<task>`).
    pub input: &'a str,
    /// The `_output.md` body — the child's delivered answer.
    pub output: &'a str,
    /// The `_meta.json` value — usage/model/exit-code metadata.
    pub metadata: &'a serde_json::Value,
    /// The `.jsonl` event lines (one NDJSON record per line).
    pub jsonl_lines: &'a [String],
}

/// Write the full artifact quadruple for one completed run (T6 orchestration helper). Ensures the
/// dir exists, then writes each of the four files gated on `cfg` — input (the task), the delivered
/// output, its metadata, and the run's NDJSON event lines. Every write is best-effort: a failed
/// artifact write must never change the run's observable result, so errors are swallowed and the
/// resolved [`ArtifactPaths`] is always returned (matching pi, whose artifact writes are likewise
/// side-effects of an already-produced `SingleResult`).
///
/// Returns `None` when `cfg.enabled` is `false` (no artifacts written), else `Some(paths)`.
pub fn write_run_artifacts(
    dir: &Path,
    run_id: &str,
    agent: &str,
    index: Option<usize>,
    content: &RunArtifactContent<'_>,
    cfg: &ArtifactConfig,
) -> Option<ArtifactPaths> {
    if !cfg.enabled {
        return None;
    }
    let paths = artifact_paths(dir, run_id, agent, index);
    if ensure_artifacts_dir(dir).is_err() {
        // If the dir cannot be created, no file can be written; still return the intended paths so a
        // caller can surface them, matching pi (which computes the paths before the dir write too).
        return Some(paths);
    }
    if cfg.include_input {
        let _ = write_artifact(&paths.input_path, content.input);
    }
    if cfg.include_output {
        let _ = write_artifact(&paths.output_path, content.output);
    }
    if cfg.include_metadata {
        let _ = write_metadata(&paths.metadata_path, content.metadata);
    }
    if cfg.include_jsonl {
        for line in content.jsonl_lines {
            let _ = append_jsonl(&paths.jsonl_path, line);
        }
        // An empty event stream still leaves a (0-byte) `.jsonl` so the quadruple is always present.
        if content.jsonl_lines.is_empty() {
            let _ = write_artifact(&paths.jsonl_path, "");
        }
    }
    Some(paths)
}

/// Build the `_meta.json` metadata value for one completed run (T6, pi
/// `persistSingleResultMetadata`, `runs/foreground/execution.ts:128-167` — and the identical `metadataPath` write in the async
/// runner, `runs/background/subagent-runner.ts:1121-1134` @v0.34.0). Carries the fields this
/// crate's [`SingleResult`] actually knows: `runId`/`agent`/`task`/`exitCode`/`usage`/`model`/
/// `attemptedModels`/`modelAttempts`/`toolCount`/`error`/`timestamp`. Pi additionally records
/// `durationMs`/`skills`/`skillsWarning`, which `SingleResult` does not carry in this crate (they
/// live on pi's richer `progressSummary`/skill-resolution shapes); those keys are omitted rather
/// than faked.
///
/// SUBA-N03 moved this out of `extension.rs` (where it was `foreground_artifact_metadata`, private
/// to the foreground path) so the detached hop-2 runner's own per-step artifact write emits the
/// BYTE-IDENTICAL metadata shape rather than a second, drifting hand-rolled one — pi likewise has
/// exactly one artifact-metadata shape shared by its foreground and async paths.
#[must_use]
pub(crate) fn run_artifact_metadata(run_id: &str, result: &SingleResult) -> serde_json::Value {
    let attempted: Vec<&str> = result
        .attempted_models
        .iter()
        .map(ModelId::as_str)
        .collect();
    let model_attempts: Vec<serde_json::Value> = result
        .model_attempts
        .iter()
        .map(|a| {
            serde_json::json!({
                "model": a.model.as_str(),
                "success": a.success,
                "exitCode": a.exit_code,
                "error": a.error,
                "usage": serde_json::to_value(&a.usage).unwrap_or(serde_json::Value::Null),
            })
        })
        .collect();
    serde_json::json!({
        "runId": run_id,
        "agent": result.agent,
        "task": result.task,
        "exitCode": result.exit_code,
        "usage": serde_json::to_value(&result.usage).unwrap_or(serde_json::Value::Null),
        "model": result.model.as_ref().map(ModelId::as_str),
        "attemptedModels": attempted,
        "modelAttempts": model_attempts,
        "toolCount": result.tool_calls.len(),
        "error": result.error,
        "timestamp": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    })
}

/// The `.jsonl` event lines for one completed run (T6). pi's `.jsonl` is the raw NDJSON the child
/// streamed to stdout; this crate's [`SingleResult`] is the already-compacted shape (it does not
/// retain the raw per-event stream — that lives transiently in the per-attempt tee under the run's
/// scratch dir, R-SA-058), so the `.jsonl` is reconstructed from the run's observable, retained
/// events: one line per summarized tool call, then a terminal `result` line. A genuine, non-empty
/// NDJSON record of the run — see the crate's T6 report for the documented divergence from pi's
/// byte-identical child stream.
///
/// SUBA-N03 moved this out of `extension.rs` for the same reason as
/// [`run_artifact_metadata`] directly above: one shape, both run paths.
#[must_use]
pub(crate) fn run_artifact_jsonl_lines(result: &SingleResult) -> Vec<String> {
    let mut lines = Vec::with_capacity(result.tool_calls.len() + 1);
    for call in &result.tool_calls {
        lines.push(
            serde_json::json!({
                "type": "tool_call",
                "text": call.text,
                "expandedText": call.expanded_text,
            })
            .to_string(),
        );
    }
    lines.push(
        serde_json::json!({
            "type": "result",
            "agent": result.agent,
            "exitCode": result.exit_code,
            "model": result.model.as_ref().map(ModelId::as_str),
            "output": result.final_output,
            "error": result.error,
        })
        .to_string(),
    );
    lines
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    /// SUBA-048 — pi `getArtifactsDir(sessionFile, projectCwd?, dirPreference = "project")`
    /// (`shared/artifacts.ts:160-183` @v0.43.0), all three arms plus their fall-throughs.
    ///
    /// THE USER ACTION: a user sets `"artifactDir": "temp"` specifically to keep generated
    /// transcripts, inputs and outputs out of a git working tree. Before the fix the resolver took
    /// three PATH arguments and NO preference, so `Some(project_cwd)` always won and `session`/
    /// `temp` were not expressible at all — the config key was accepted and inert.
    #[test]
    fn the_artifact_dir_preference_selects_pis_three_arms() {
        let session_file = Path::new("/sessions/abc/session.jsonl");
        let project = Path::new("/repo");
        let temp_cwd = Path::new("/repo");
        let session_sibling = Path::new("/sessions/abc").join(SESSION_ARTIFACTS_SUBDIR);

        // `temp` ignores BOTH a project cwd and a session file.
        assert_eq!(
            resolve_artifacts_dir(
                Some(session_file),
                Some(project),
                temp_cwd,
                ArtifactDirPreference::Temp
            ),
            temp_artifacts_dir(temp_cwd)
        );
        // `session` ignores the project cwd...
        assert_eq!(
            resolve_artifacts_dir(
                Some(session_file),
                Some(project),
                temp_cwd,
                ArtifactDirPreference::Session
            ),
            session_sibling
        );
        // ...and falls back to temp when there is no session file (pi's `return TEMP_ARTIFACTS_DIR`).
        assert_eq!(
            resolve_artifacts_dir(
                None,
                Some(project),
                temp_cwd,
                ArtifactDirPreference::Session
            ),
            temp_artifacts_dir(temp_cwd)
        );
        // `project` is upstream's default and keeps the previous three-way fall-through.
        assert_eq!(
            resolve_artifacts_dir(
                Some(session_file),
                Some(project),
                temp_cwd,
                ArtifactDirPreference::Project
            ),
            project_artifacts_dir(project)
        );
        assert_eq!(
            resolve_artifacts_dir(
                Some(session_file),
                None,
                temp_cwd,
                ArtifactDirPreference::Project
            ),
            session_sibling
        );
        assert_eq!(
            resolve_artifacts_dir(None, None, temp_cwd, ArtifactDirPreference::Project),
            temp_artifacts_dir(temp_cwd)
        );
        // The default IS `project` (pi `DEFAULT_ARTIFACT_CONFIG.dir`).
        assert_eq!(
            ArtifactDirPreference::default(),
            ArtifactDirPreference::Project
        );
    }

    /// SUBA-048 / PARITY-GAPS PB-13 — pi `getChainRunsDir(projectCwd, dirPreference = "project")`
    /// (`shared/artifacts.ts:145-158` @v0.43.0): only `project` goes to the project tree, and
    /// `session` shares `temp`'s arm rather than getting a session sibling of its own.
    ///
    /// RED before the fix: there was no `resolve_chain_runs_dir` at all, `project_chain_runs_dir`
    /// had zero references anywhere in the crate, and every chain run's artifacts went to the temp
    /// root regardless of configuration.
    #[test]
    fn the_chain_runs_root_follows_pis_two_arm_preference_split() {
        let cwd = Path::new("/repo");
        assert_eq!(
            resolve_chain_runs_dir(cwd, ArtifactDirPreference::Project),
            project_chain_runs_dir(cwd),
            "upstream's default arm returns getProjectChainRunsDir"
        );
        assert_eq!(
            resolve_chain_runs_dir(cwd, ArtifactDirPreference::Session),
            chain_runs_dir(cwd),
            "upstream folds `session` into the temp arm for chain runs"
        );
        assert_eq!(
            resolve_chain_runs_dir(cwd, ArtifactDirPreference::Temp),
            chain_runs_dir(cwd)
        );
        // The default preference therefore puts chain runs INSIDE the project, which is the whole
        // of PB-13's observable claim.
        assert_eq!(
            resolve_chain_runs_dir(cwd, ArtifactDirPreference::default()),
            project_subagents_dir(cwd).join(CHAIN_RUNS_SUBDIR)
        );
    }

    /// SUBA-048's validation half — pi `extension/config.ts:22-24,51-53` THROWS on an unsupported
    /// value where cyrup silently ignored a good one.
    #[test]
    fn an_unsupported_artifact_dir_is_refused_with_pis_text() {
        for ok in ["project", "session", "temp"] {
            assert!(ArtifactDirPreference::parse(ok).is_ok(), "{ok}");
        }
        assert_eq!(
            ArtifactDirPreference::parse("Project").expect_err("case-sensitive, like upstream"),
            r#"config.artifactDir must be "project", "session", or "temp""#
        );
        assert_eq!(
            ArtifactDirPreference::parse("").expect_err("empty is not a preference"),
            r#"config.artifactDir must be "project", "session", or "temp""#
        );
    }

    /// SUBA-059 — pi `if (maxAgeDays <= 0 || !fs.existsSync(dir)) return;`
    /// (`shared/artifacts.ts:231` @v0.47.1). `cleanupDays: 0` DISABLES the sweep.
    ///
    /// Red before the fix in the worst possible direction: `cutoff = now - 0 * ONE_DAY_MS` is
    /// `now`, so every artifact's mtime was below it and a user asking to keep everything would
    /// have had everything deleted.
    #[test]
    fn a_zero_cleanup_horizon_disables_the_sweep_instead_of_purging_everything() {
        let dir = tempfile::tempdir().expect("tempdir");
        let victim = dir.path().join("run_agent_output.md");
        std::fs::write(&victim, "the record of what the fan-out did").expect("write");

        cleanup_old_artifacts(dir.path(), 0);
        assert!(
            victim.exists(),
            "cleanupDays: 0 must disable the sweep, not delete everything"
        );
        // ...and it must not even write the throttle marker, since it returned before doing work.
        assert!(!dir.path().join(CLEANUP_MARKER_FILE).exists());
    }

    #[test]
    fn artifact_paths_match_pi_getartifactpaths_naming() {
        let dir = Path::new("/tmp/arts");
        // No index → no `_<i>` suffix (pi `suffix = index !== undefined ? ...`).
        let p = artifact_paths(dir, "run123", "reviewer", None);
        assert_eq!(p.input_path, dir.join("run123_reviewer_input.md"));
        assert_eq!(p.output_path, dir.join("run123_reviewer_output.md"));
        assert_eq!(p.jsonl_path, dir.join("run123_reviewer.jsonl"));
        assert_eq!(p.metadata_path, dir.join("run123_reviewer_meta.json"));

        // With index → `_<i>` between agent and the file-kind suffix.
        let pi_idx = artifact_paths(dir, "run123", "reviewer", Some(2));
        assert_eq!(pi_idx.input_path, dir.join("run123_reviewer_2_input.md"));
        assert_eq!(pi_idx.jsonl_path, dir.join("run123_reviewer_2.jsonl"));
    }

    #[test]
    fn safe_agent_replaces_non_word_chars_like_pi() {
        // pi `agent.replace(/[^\w.-]/g, "_")`: keep [A-Za-z0-9_.-], everything else → `_`.
        assert_eq!(safe_agent("code-analysis.custom"), "code-analysis.custom");
        assert_eq!(safe_agent("weird agent/name!"), "weird_agent_name_");
        assert_eq!(safe_agent("a_b.c-d"), "a_b.c-d");
    }

    #[test]
    fn default_config_matches_pi_default_artifact_config() {
        let c = ArtifactConfig::default();
        assert!(c.enabled && c.include_input && c.include_output && c.include_metadata);
        assert!(
            !c.include_jsonl,
            "pi DEFAULT_ARTIFACT_CONFIG.includeJsonl is false"
        );
        assert_eq!(c.cleanup_days, 7);
        // The foreground variant additionally captures the event stream.
        assert!(ArtifactConfig::foreground().include_jsonl);
    }

    #[test]
    fn write_run_artifacts_writes_the_full_quadruple_when_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let art_dir = dir.path().join("arts");
        let meta = serde_json::json!({ "runId": "r1", "agent": "worker", "exitCode": 0 });
        let jsonl = ["{\"type\":\"final\"}".to_string()];
        let paths = write_run_artifacts(
            &art_dir,
            "r1",
            "worker",
            None,
            &RunArtifactContent {
                input: "# Task for worker\n\ndo the thing",
                output: "the answer",
                metadata: &meta,
                jsonl_lines: &jsonl,
            },
            &ArtifactConfig::foreground(),
        )
        .expect("enabled config writes artifacts");

        for p in [
            &paths.input_path,
            &paths.output_path,
            &paths.jsonl_path,
            &paths.metadata_path,
        ] {
            assert!(
                p.exists(),
                "expected artifact file to exist: {}",
                p.display()
            );
        }
        assert_eq!(
            std::fs::read_to_string(&paths.output_path).unwrap(),
            "the answer"
        );
        assert!(
            std::fs::read_to_string(&paths.input_path)
                .unwrap()
                .contains("do the thing")
        );
        assert!(
            std::fs::read_to_string(&paths.metadata_path)
                .unwrap()
                .contains("\"exitCode\": 0")
        );
        assert!(
            std::fs::read_to_string(&paths.jsonl_path)
                .unwrap()
                .contains("\"type\":\"final\"")
        );
    }

    #[test]
    fn write_run_artifacts_omits_jsonl_under_pi_default_config() {
        let dir = tempfile::tempdir().unwrap();
        let meta = serde_json::json!({});
        let paths = write_run_artifacts(
            dir.path(),
            "r2",
            "worker",
            None,
            &RunArtifactContent {
                input: "in",
                output: "out",
                metadata: &meta,
                jsonl_lines: &[],
            },
            &ArtifactConfig::default(),
        )
        .unwrap();
        assert!(
            paths.input_path.exists() && paths.output_path.exists() && paths.metadata_path.exists()
        );
        assert!(
            !paths.jsonl_path.exists(),
            "pi default leaves the .jsonl unwritten"
        );
    }

    #[test]
    fn cleanup_removes_old_files_but_keeps_fresh_ones() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old_output.md");
        let fresh = dir.path().join("fresh_output.md");
        std::fs::write(&old, "old").unwrap();
        std::fs::write(&fresh, "fresh").unwrap();

        // Backdate `old`'s mtime to 10 days ago (older than the 7-day horizon).
        let ten_days_ago = SystemTime::now() - std::time::Duration::from_secs(10 * 24 * 60 * 60);
        let ft = filetime::FileTime::from_system_time(ten_days_ago);
        filetime::set_file_mtime(&old, ft).unwrap();

        cleanup_old_artifacts(dir.path(), DEFAULT_CLEANUP_DAYS);

        assert!(
            !old.exists(),
            "a 10-day-old artifact is swept under the 7-day horizon"
        );
        assert!(fresh.exists(), "a fresh artifact survives the sweep");
        assert!(
            dir.path().join(CLEANUP_MARKER_FILE).exists(),
            "the throttle marker is written"
        );
    }

    #[test]
    fn cleanup_is_throttled_by_a_fresh_marker() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old_output.md");
        std::fs::write(&old, "old").unwrap();
        let ten_days_ago = SystemTime::now() - std::time::Duration::from_secs(10 * 24 * 60 * 60);
        filetime::set_file_mtime(&old, filetime::FileTime::from_system_time(ten_days_ago)).unwrap();

        // A marker touched "now" must short-circuit the sweep entirely (pi 24h throttle).
        std::fs::write(
            dir.path().join(CLEANUP_MARKER_FILE),
            now_millis_u128().to_string(),
        )
        .unwrap();

        cleanup_old_artifacts(dir.path(), DEFAULT_CLEANUP_DAYS);
        assert!(
            old.exists(),
            "a fresh throttle marker skips the sweep, so the old file survives"
        );
    }
}
