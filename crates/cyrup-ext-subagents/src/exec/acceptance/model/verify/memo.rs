//! G80 per-workspace memoization of verify results (pi `acceptance.ts:1032-1132`): the
//! workspace-state identity a memoized result is keyed on.

use std::path::{Path, PathBuf};

use super::super::types::{
    AcceptanceVerifyCommand, AcceptanceVerifyResult, VerifyWorkspaceKind, VerifyWorkspaceState,
};
use super::super::verify::redact::effective_verify_env;
use super::super::verify::run::{
    DEFAULT_VERIFY_TIMEOUT_MS, resolve_verify_cwd, run_verify_command_with_cancel,
};

// --------------------------------------------------------------------------------------------
// G80: per-workspace memoization of verify results (acceptance.ts:1032-1132)
// --------------------------------------------------------------------------------------------

/// `hash` (`acceptance.ts:1034-1036`): lowercase hex sha256.
#[must_use]
fn hash_bytes(value: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value);
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The two values upstream requires BOTH of before it will memoize anything at all
/// (`if (!workspaceState || !options.artifactsDir || !options.runId)`, `acceptance.ts:1085`):
/// the run's artifacts root and the run id that scopes the cache within it.
///
/// Passing `None` for the whole context is upstream's "no artifacts configured" case — every
/// verify command then executes for real, exactly as it did before memoization existed. That
/// is also what pi's chain-execution group gate does: its two `evaluateAcceptance` calls
/// (`chain-execution.ts:1037-1046,1233-1242`) pass neither field.
#[derive(Debug, Clone, Copy)]
pub struct VerifyMemoContext<'a> {
    /// pi `options.artifactsDir` — the run's artifacts root. Memo artifacts land under
    /// `<artifacts_dir>/acceptance/verify/<run_id>/<cacheKey>.json` (`acceptance.ts:1102`).
    pub artifacts_dir: &'a Path,
    /// pi `options.runId`.
    pub run_id: &'a str,
}

/// `readVerifyWorkspaceState` (`acceptance.ts:1046-1060`): identify the git working tree
/// `cwd` sits in, as `HEAD` plus a hash of the full uncommitted diff.
///
/// Returns `None` — which disables memoization for this command entirely — when `cwd` is not
/// inside a git checkout, when either `git` invocation fails, or when `HEAD` is empty (an
/// unborn branch). A non-git workspace has no cheap identity to key a cache on, so upstream
/// declines to guess one.
///
/// **[CYRUP-DELTA: mechanism]** upstream uses `spawnSync` and hashes the diff after decoding it
/// as UTF-8; this awaits `tokio::process::Command` (blocking the async executor on three git
/// invocations is not an option here) and hashes the diff's RAW BYTES, which is strictly more
/// faithful for the `--binary` diffs the flag exists to produce — a lossy decode would collapse
/// distinct binary blobs onto the same replacement characters and therefore the same key.
pub async fn read_verify_workspace_state(cwd: &Path) -> Option<VerifyWorkspaceState> {
    let repo = tokio::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .await
        .ok()?;
    if !repo.status.success() {
        return Option::None;
    }
    let repo_root_raw = String::from_utf8(repo.stdout).ok()?;
    let repo_root_raw = repo_root_raw.trim();
    if repo_root_raw.is_empty() {
        return Option::None;
    }
    // `fs.realpathSync` (`acceptance.ts:1049`) — both sides are canonicalized so the
    // `path.relative` below cannot be defeated by a symlinked cwd.
    let repo_root = std::fs::canonicalize(repo_root_raw).ok()?;

    let head = tokio::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo_root)
        .output()
        .await
        .ok()?;
    let diff = tokio::process::Command::new("git")
        .args(["diff", "--binary", "--full-index", "HEAD", "--"])
        .current_dir(&repo_root)
        .output()
        .await
        .ok()?;
    if !head.status.success() || !diff.status.success() {
        return Option::None;
    }
    let head_text = String::from_utf8(head.stdout).ok()?;
    let head_text = head_text.trim();
    if head_text.is_empty() {
        return Option::None;
    }

    // `path.relative(repoRoot, fs.realpathSync(cwd)) || "."` (`acceptance.ts:1056`). `cwd` is
    // always inside `repoRoot` here — `repoRoot` was derived by running `rev-parse` FROM it —
    // so a plain prefix strip is exactly `path.relative`, and `""` becomes `"."`.
    let cwd_real = std::fs::canonicalize(cwd).ok()?;
    let relative = cwd_real.strip_prefix(&repo_root).ok()?;
    let cwd_relative = if relative.as_os_str().is_empty() {
        ".".to_string()
    } else {
        relative.to_string_lossy().into_owned()
    };

    Some(VerifyWorkspaceState {
        kind: VerifyWorkspaceKind::GitTracked,
        repo_root: repo_root.to_string_lossy().into_owned(),
        cwd_relative,
        head: head_text.to_string(),
        diff_hash: hash_bytes(&diff.stdout),
    })
}

/// The shape marker stamped into a memo artifact's `resultShape` field.
///
/// **[CYRUP-DELTA: mechanism]** upstream has exactly ONE verify-result type, so its artifact
/// needs no discriminant, and as of the verify-layer collapse so does this crate — every writer
/// and every reader is [`AcceptanceVerifyResult`]. The marker survives because artifacts
/// written by the previous build DO carry a second value (`"verify-command-result"`, the
/// retired lattice shape), and those must read as a clean MISS — re-run the command — rather
/// than be coerced through serde into a shape they were never written as. It is an opaque field
/// of a private artifact; nothing observable depends on it.
const MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT: &str = "acceptance-verify-result";

/// The everything-but-the-result half of a memo artifact, shared by both result shapes.
///
/// Mirrors upstream's written object (`acceptance.ts:1115-1126`) field for field, minus
/// `result` (supplied by the caller) plus `resultShape` (see
/// [`MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT`]).
pub(crate) struct MemoIdentity {
    pub(crate) cache_key: String,
    pub(crate) artifact_path: PathBuf,
    pub(crate) env_keys: Vec<String>,
    pub(crate) env_hash: String,
    pub(crate) timeout_ms: u64,
    pub(crate) allow_failure: bool,
    pub(crate) workspace_state: VerifyWorkspaceState,
}

impl MemoIdentity {
    /// `acceptance.ts:1088-1102`: derive this command's cache key and artifact path against an
    /// already-read [`VerifyWorkspaceState`].
    ///
    /// The key covers everything that can change the command's OUTCOME: its text, the
    /// repo-relative directory it runs in, the names of the env keys it declares, a hash of the
    /// entire effective environment, its timeout, its `allowFailure` flag, `HEAD`, and the
    /// working-tree diff hash. Note `env_keys` records only NAMES (the ledger is
    /// transcript-visible) while `env_hash` covers every VALUE — that split is upstream's, and
    /// it is what lets a rotated credential invalidate the memo without ever being written
    /// down.
    ///
    /// **[CYRUP-DELTA: mechanism]** the key is a sha256 over `serde_json`'s rendering of the
    /// same field set rather than over V8's `JSON.stringify` of it, so the digest VALUE differs
    /// from pi's. Nothing compares the two: a cache key is only ever matched against another
    /// key produced by the same build, and upstream re-checks `cached.cacheKey === cacheKey`
    /// on read for exactly that reason.
    pub(crate) fn derive(
        command: &AcceptanceVerifyCommand,
        memo: VerifyMemoContext<'_>,
        workspace_state: VerifyWorkspaceState,
        result_shape: &str,
    ) -> Self {
        // `Object.keys(command.env ?? {}).sort()` (`acceptance.ts:1088`) — a `BTreeMap` is
        // already sorted.
        let env_keys: Vec<String> = command
            .env
            .as_ref()
            .map(|env| env.keys().cloned().collect())
            .unwrap_or_default();
        // `hash(JSON.stringify(<effective env, key-sorted>))` (`acceptance.ts:1089`).
        let effective = effective_verify_env(command.env.as_ref());
        let env_hash = hash_bytes(
            serde_json::to_string(&effective)
                .unwrap_or_default()
                .as_bytes(),
        );
        let timeout_ms = command.timeout_ms.unwrap_or(DEFAULT_VERIFY_TIMEOUT_MS);
        let allow_failure = command.allow_failure == Some(true);
        let key_material = serde_json::json!({
            "version": 1,
            "command": command.command,
            "cwdRelative": workspace_state.cwd_relative,
            "envKeys": env_keys,
            "envHash": env_hash,
            "timeoutMs": timeout_ms,
            "allowFailure": allow_failure,
            "head": workspace_state.head,
            "diffHash": workspace_state.diff_hash,
            "resultShape": result_shape,
        });
        let cache_key = hash_bytes(
            serde_json::to_string(&key_material)
                .unwrap_or_default()
                .as_bytes(),
        );
        // `path.join(artifactsDir, "acceptance", "verify", runId, `${cacheKey}.json`)`
        // (`acceptance.ts:1102`).
        let artifact_path = memo
            .artifacts_dir
            .join("acceptance")
            .join("verify")
            .join(memo.run_id)
            .join(format!("{cache_key}.json"));
        Self {
            cache_key,
            artifact_path,
            env_keys,
            env_hash,
            timeout_ms,
            allow_failure,
            workspace_state,
        }
    }

    /// The `result` payload of a matching memo artifact, or `None` for any miss.
    ///
    /// Upstream's read is wrapped in a bare `try {} catch {}` whose comment says it out loud —
    /// *"A cache miss or unreadable artifact must not prevent host verification"*
    /// (`acceptance.ts:1108-1110`) — so an absent file, malformed JSON, a stale `cacheKey` or a
    /// foreign `resultShape` are all just misses.
    pub(crate) fn read_cached(&self, result_shape: &str) -> Option<serde_json::Value> {
        let raw = std::fs::read(&self.artifact_path).ok()?;
        let cached: serde_json::Value = serde_json::from_slice(&raw).ok()?;
        if cached.get("cacheKey").and_then(serde_json::Value::as_str)
            != Some(self.cache_key.as_str())
        {
            return Option::None;
        }
        // An artifact with no marker predates the field; treat it as the pi-shaped default so
        // this stays a pure addition.
        let shape = cached
            .get("resultShape")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT);
        if shape != result_shape {
            return Option::None;
        }
        cached.get("result").cloned()
    }

    /// Write the memo artifact (`acceptance.ts:1113-1126`), returning the error TEXT upstream
    /// puts on `artifactError` when the write fails.
    ///
    /// Best-effort by construction: the command has already run and its real exit code is
    /// already known, so a failure here can only cost a future re-run, never a wrong verdict.
    pub(crate) fn write_cached(
        &self,
        command: &AcceptanceVerifyCommand,
        result_shape: &str,
        result: &serde_json::Value,
    ) -> Result<(), String> {
        let payload = serde_json::json!({
            "version": 1,
            "cacheKey": self.cache_key,
            "command": command.command,
            "cwdRelative": self.workspace_state.cwd_relative,
            "envKeys": self.env_keys,
            "envHash": self.env_hash,
            "timeoutMs": self.timeout_ms,
            "allowFailure": self.allow_failure,
            "workspaceState": self.workspace_state,
            "resultShape": result_shape,
            "result": result,
        });
        let text = serde_json::to_string_pretty(&payload).map_err(|err| err.to_string())?;
        if let Some(parent) = self.artifact_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        std::fs::write(&self.artifact_path, text).map_err(|err| err.to_string())
    }
}

/// `runMemoizedVerifyCommand` (`acceptance.ts:1072-1132`): replay a verify command's recorded
/// result when the workspace has not changed since it was recorded, otherwise run it for real
/// and record the outcome.
///
/// Falls straight through to [`crate::exec::acceptance::model::verify::run::run_verify_command`] — no artifact read, no artifact write, no
/// evidence fields — whenever there is no memo context or the cwd is not a git working tree
/// (`acceptance.ts:1085-1087`). The memoized replay carries the recorded `exitCode`, `status`,
/// `stdout`, `stderr` and `durationMs` but re-stamps `id`/`command`/`cwd` from the CURRENT
/// command (`acceptance.ts:1106`), so a renamed criterion id still reports under its new name.
pub async fn run_memoized_verify_command(
    command: &AcceptanceVerifyCommand,
    default_cwd: &Path,
    memo: Option<VerifyMemoContext<'_>>,
) -> AcceptanceVerifyResult {
    run_memoized_verify_command_with_cancel(
        command,
        default_cwd,
        memo,
        &cyrup_core::CancelToken::new(),
    )
    .await
}

/// SUBA-028 — [`run_memoized_verify_command`] with the caller's cancellation token (pi's
/// `options.signal`, forwarded to `runVerifyCommand` at `acceptance.ts:1130`).
///
/// A memo HIT is unaffected by cancellation and deliberately so: it spawns nothing, so there is
/// nothing to abort and returning the recorded result is strictly faster than checking.
pub async fn run_memoized_verify_command_with_cancel(
    command: &AcceptanceVerifyCommand,
    default_cwd: &Path,
    memo: Option<VerifyMemoContext<'_>>,
    cancel: &cyrup_core::CancelToken,
) -> AcceptanceVerifyResult {
    let cwd = resolve_verify_cwd(command, default_cwd);
    let Some(memo) = memo else {
        return run_verify_command_with_cancel(command, default_cwd, cancel).await;
    };
    // `try { workspaceState = readVerifyWorkspaceState(cwd) } catch { undefined }`
    // (`acceptance.ts:1079-1084`).
    let Some(workspace_state) = read_verify_workspace_state(&cwd).await else {
        return run_verify_command_with_cancel(command, default_cwd, cancel).await;
    };
    let identity = MemoIdentity::derive(
        command,
        memo,
        workspace_state,
        MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT,
    );

    if let Some(cached) = identity.read_cached(MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT)
        // `isCachedVerifyResult` (`acceptance.ts:1062-1070`) asserts id/command are strings,
        // `exitCode` is a number or an explicit null, `status` is one of the four literals and
        // `durationMs` is a number. Every one of those is a REQUIRED field of
        // `AcceptanceVerifyResult` (only `cwd` and the evidence fields carry `#[serde(default)]`),
        // so a successful deserialization IS that predicate.
        && let Ok(result) = serde_json::from_value::<AcceptanceVerifyResult>(cached)
    {
        return AcceptanceVerifyResult {
            id: command.id.clone(),
            command: command.command.clone(),
            cwd: Some(cwd.display().to_string()),
            artifact_path: Some(identity.artifact_path.display().to_string()),
            cache_key: Some(identity.cache_key.clone()),
            memoized: Some(true),
            env_keys: Some(identity.env_keys.clone()),
            env_hash: Some(identity.env_hash.clone()),
            workspace_state: Some(identity.workspace_state.clone()),
            artifact_error: Option::None,
            ..result
        };
    }

    let result = run_verify_command_with_cancel(command, default_cwd, cancel).await;
    let mut evidenced = AcceptanceVerifyResult {
        artifact_path: Some(identity.artifact_path.display().to_string()),
        cache_key: Some(identity.cache_key.clone()),
        memoized: Some(false),
        env_keys: Some(identity.env_keys.clone()),
        env_hash: Some(identity.env_hash.clone()),
        workspace_state: Some(identity.workspace_state.clone()),
        artifact_error: Option::None,
        ..result
    };
    let payload = serde_json::to_value(&evidenced).unwrap_or(serde_json::Value::Null);
    if let Err(message) =
        identity.write_cached(command, MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT, &payload)
    {
        // `evidenced.artifactError = …; delete evidenced.artifactPath;`
        // (`acceptance.ts:1128-1129`) — never claim an artifact that is not there.
        evidenced.artifact_error = Some(message);
        evidenced.artifact_path = Option::None;
    }
    evidenced
}
