//! CFG-067 — the RUN FAN-OUT BUDGET: the Rust port of
//! `pi-subagents/src/runs/shared/run-fanout-budget.ts` @v0.64.0, plus its limit resolver
//! (`resolveMaxSubagentSpawnsPerRun`, `shared/types.ts:2807-2818` @v0.64.0).
//!
//! # What it bounds, and why it is not [`crate::exec::spawn_budget`]
//!
//! [`crate::exec::spawn_budget`] is the PER-SESSION cap (`PI_SUBAGENT_MAX_SPAWNS_PER_SESSION`): a
//! counter living in this process's memory, keyed by session id, reset when the session changes.
//! It cannot bound a subtree, because every re-exec'd child is a fresh process with a fresh
//! counter — a run that spawns a child that spawns a child pays once per process, not once per
//! subtree.
//!
//! The run fan-out budget is the PER-RUN cap and it is the tighter one. Its state is a DIRECTORY,
//! not a counter: one manifest naming the root run and its limit, and one `claims/NNNNNN.json`
//! file per admitted child. Because the state is on disk and its descriptor crosses the spawn
//! boundary in [`RUN_FANOUT_BUDGET_ENV`], every process in one run's subtree claims against the
//! SAME ledger. That is the whole property: a run cannot walk around its cap by delegating.
//!
//! Claims are never released (upstream's doctor line says so in as many words:
//! `extension/doctor.ts:184` @v0.64.0, "cumulative claims are never released; a new top-level run
//! creates a new budget"). A fan-out that fails still spent its share, exactly as
//! [`crate::exec::spawn_budget`]'s reservation is never refunded.
//!
//! # Admission is all-or-nothing
//!
//! [`claim_run_fanout_batch_in`] takes the whole batch of paths a dispatch wants to start. Under the
//! directory's admission lock it compares `paths.len()` against what remains and refuses the
//! ENTIRE batch if it does not fit, having created no claim files ("No children from this
//! admission group were started", the rejection text at `run-fanout-budget.ts:279`). A per-child
//! loop would admit a prefix of the batch and leave the caller to unwind half-started children.
//!
//! # Rust deltas
//!
//! * `[CYRUP-DELTA]` — upstream throws `RunFanoutLimitError` (a subclass) for the cap and a plain
//!   `Error` for everything else, and callers discriminate with `instanceof`
//!   (`foreground/subagent-executor.ts:62`). Rust has no subclassing, so the discriminant is the
//!   [`RunFanoutError`] enum: [`RunFanoutError::Limit`] carries the structured
//!   [`RunFanoutRejection`] a caller reports to the model, [`RunFanoutError::Invalid`] carries the
//!   verbatim message of every other upstream `throw`. `Display` reproduces upstream's text in
//!   both arms, so a caller that only formats the error is byte-identical to pi.
//! * `[CYRUP-DELTA]` — upstream's stale-lock probe is `process.kill(owner.pid, 0)` and treats an
//!   `EPERM` as "alive" (`:157`). This uses `nix::sys::signal::kill(pid, None)` and maps `EPERM`
//!   the same way; the crate is `#![forbid(unsafe_code)]`, so `libc::kill` is not reachable.
//! * `[CYRUP-DELTA]` — upstream's retry ladder is clamped by `PI_SUBAGENT_FS_RETRY_MAX_TOTAL_MS`
//!   (`shared/file-system-retry.ts`), which is NOT one of the twelve names `CFG-067` enumerates
//!   and is not ported here; [`FILE_SYSTEM_RETRY_DELAYS_MS`] is upstream's unclamped base ladder.
//!   The lock wait is therefore pi's default, and only its default.
//! * The waiter is injected ([`claim_run_fanout_batch_with_commit_in`]'s `wait` argument) rather
//!   than read from a module-level global, so a contention test does not sleep for eight seconds
//!   and so an async caller can supply its own parking strategy. The blocking default matches
//!   upstream's `Atomics.wait`; an async caller must reach this module through
//!   `tokio::task::spawn_blocking`, which is stated on every blocking entry point.
//! * Every filesystem root is a parameter of an `_in` function, with a shell that supplies
//!   [`run_fanout_root`]. Nothing in the rules reads the process environment or the real temp
//!   root, so every branch below is provable without mutating process-global state — the same
//!   convention `background::temp_root_dir_from` and `exec::tool_budget` already follow.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// pi `RUN_FANOUT_BUDGET_ENV = "PI_SUBAGENT_RUN_FANOUT_BUDGET"` (`run-fanout-budget.ts:12`), in
/// this crate's `CYRUP_` naming family — the rename convention
/// [`crate::exec::spawn_budget::MAX_SPAWNS_PER_SESSION_ENV`] documents.
///
/// Carries a base64url JSON [`RunFanoutBudgetDescriptor`], written into a child's environment only
/// when that child is fan-out authorized (pi `pi-args.ts:942-943`).
pub const RUN_FANOUT_BUDGET_ENV: &str = "CYRUP_SUBAGENT_RUN_FANOUT_BUDGET";

/// The upstream spelling of [`RUN_FANOUT_BUDGET_ENV`], honoured as a read-side compatibility alias
/// so a subtree launched by a pi parent keeps claiming against the ledger that parent created.
pub const RUN_FANOUT_BUDGET_ENV_PI_ALIAS: &str = "PI_SUBAGENT_RUN_FANOUT_BUDGET";

/// pi `PI_SUBAGENT_MAX_SPAWNS_PER_RUN` (`shared/types.ts:2815`), in the `CYRUP_` family. The
/// PER-RUN sibling of [`crate::exec::spawn_budget::MAX_SPAWNS_PER_SESSION_ENV`].
pub const MAX_SPAWNS_PER_RUN_ENV: &str = "CYRUP_SUBAGENT_MAX_SPAWNS_PER_RUN";

/// The upstream spelling of [`MAX_SPAWNS_PER_RUN_ENV`], honoured as a read-side compatibility
/// alias.
pub const MAX_SPAWNS_PER_RUN_ENV_PI_ALIAS: &str = "PI_SUBAGENT_MAX_SPAWNS_PER_RUN";

/// pi `DEFAULT_MAX_SUBAGENT_SPAWNS_PER_RUN = 64` (`shared/types.ts:2807`).
pub const DEFAULT_MAX_SPAWNS_PER_RUN: u32 = 64;

/// pi `ADMISSION_LOCK_STALE_MS = 60_000` (`run-fanout-budget.ts:146`).
const ADMISSION_LOCK_STALE_MS: u64 = 60_000;

/// pi `BASE_FILE_SYSTEM_RETRY_DELAYS_MS` (`shared/file-system-retry.ts`), indexed by attempt
/// number; running off the end is "timed out acquiring lock" (`run-fanout-budget.ts:191`).
pub const FILE_SYSTEM_RETRY_DELAYS_MS: [u64; 9] = [10, 25, 50, 100, 200, 500, 1000, 2000, 4000];

/// pi's `safeRootRunId` cap (`run-fanout-budget.ts:41`).
const MAX_ROOT_RUN_ID_CHARS: usize = 120;

/// pi `RunFanoutBudgetDescriptor` (`shared/types.ts:760-766`) — the portable handle to one run's
/// ledger. This is what is written into an async run's directory, what is base64url-encoded into
/// [`RUN_FANOUT_BUDGET_ENV`], and what every claim is validated against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFanoutBudgetDescriptor {
    /// pi `version` — always `1`; a decode of anything else is refused.
    pub version: u32,
    /// pi `rootRunId` — the TOP-LEVEL run this ledger belongs to, not the nearest parent.
    pub root_run_id: String,
    /// pi `directory` — the ledger directory, always under [`run_fanout_root`].
    pub directory: PathBuf,
    /// pi `limit` — a positive integer; `0` is not "unlimited" here (unlike the per-session cap),
    /// it is invalid, because a fan-out ledger with no bound has nothing to record.
    pub limit: u32,
    /// pi `parentPath?` — the claim-path prefix a nested process qualifies its own claims with, so
    /// `chain[0].expand[2]` under a parent `workflow[a]` records as `workflow[a]/chain[0]…`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_path: Option<String>,
}

/// pi `RunFanoutBudgetSnapshot` (`shared/types.ts:768-772`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFanoutBudgetSnapshot {
    /// Claims already recorded on disk.
    pub used: u32,
    /// The manifest's limit.
    pub limit: u32,
    /// `limit - used`, floored at zero.
    pub remaining: u32,
}

/// pi `RunFanoutRejection` (`shared/types.ts:774-778`) — the structured refusal a caller reports
/// back, carrying the snapshot the decision was taken against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFanoutRejection {
    /// pi `code: "RUN_FANOUT_LIMIT"` — a literal type upstream, so it is not a field a caller
    /// chooses; [`RunFanoutRejection::CODE`] is the only value this crate ever writes.
    pub code: String,
    /// The first claim path that did NOT fit (`qualified[before.remaining]`).
    pub path: String,
    /// How many claims the refused batch asked for.
    pub requested: u32,
    /// Claims already recorded when the batch was refused.
    pub used: u32,
    /// The manifest's limit.
    pub limit: u32,
    /// What was left when the batch was refused.
    pub remaining: u32,
}

impl RunFanoutRejection {
    /// pi's literal `code` (`shared/types.ts:775`).
    pub const CODE: &'static str = "RUN_FANOUT_LIMIT";
}

/// The discriminated failure of every operation in this module.
///
/// `[CYRUP-DELTA]` — upstream's `RunFanoutLimitError extends Error`
/// (`run-fanout-budget.ts:29-37`) versus its plain `throw new Error(...)`; the `instanceof` check
/// callers use becomes this enum's match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunFanoutError {
    /// pi `RunFanoutLimitError` — the batch did not fit. Nothing was claimed.
    Limit(Box<RunFanoutRejection>),
    /// Every other upstream `throw new Error(...)`, carrying its verbatim message.
    Invalid(String),
}

impl std::fmt::Display for RunFanoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Limit(rejection) => f.write_str(&format_run_fanout_rejection(rejection)),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RunFanoutError {}

impl RunFanoutError {
    /// The [`RunFanoutBudgetSnapshot`] a limit refusal was taken against (upstream's
    /// `RunFanoutLimitError.snapshot`, `run-fanout-budget.ts:35`); `None` for every other arm.
    #[must_use]
    pub fn snapshot(&self) -> Option<RunFanoutBudgetSnapshot> {
        match self {
            Self::Limit(rejection) => Some(RunFanoutBudgetSnapshot {
                used: rejection.used,
                limit: rejection.limit,
                remaining: rejection.remaining,
            }),
            Self::Invalid(_) => None,
        }
    }
}

fn invalid(message: impl Into<String>) -> RunFanoutError {
    RunFanoutError::Invalid(message.into())
}

/// pi `RUN_FANOUT_ROOT = path.join(TEMP_ROOT_DIR, "run-fanout-budgets")`
/// (`run-fanout-budget.ts:13`), on cyrup's own `TEMP_ROOT_DIR`
/// ([`crate::background::temp_root_dir`]).
#[must_use]
pub fn run_fanout_root() -> PathBuf {
    crate::background::temp_root_dir().join("run-fanout-budgets")
}

/// pi `resolveMaxSubagentSpawnsPerRun` (`shared/types.ts:2814-2818`): env override, then config,
/// then [`DEFAULT_MAX_SPAWNS_PER_RUN`].
///
/// Unlike the per-session sibling ([`crate::exec::spawn_budget::resolve_max_spawns_per_session`]),
/// `0` is NOT "unlimited" on either surface — pi's `normalizeMaxSubagentSpawnsPerRun`
/// (`:2809-2812`) drops any value that is not `> 0`, so `0` and a typo both fall through to the
/// next rung and the run is bounded either way. That asymmetry is deliberate upstream and is the
/// reason this resolver is not a wrapper around the session one.
#[must_use]
pub fn resolve_max_spawns_per_run(configured: Option<u32>) -> u32 {
    resolve_max_spawns_per_run_with(&|key| std::env::var(key).ok(), configured)
}

/// The pure core of [`resolve_max_spawns_per_run`], with the environment injected.
#[must_use]
pub fn resolve_max_spawns_per_run_with(
    get: &dyn Fn(&str) -> Option<String>,
    configured: Option<u32>,
) -> u32 {
    let from_env = get(MAX_SPAWNS_PER_RUN_ENV)
        .or_else(|| get(MAX_SPAWNS_PER_RUN_ENV_PI_ALIAS))
        .and_then(|raw| normalize_max_spawns_per_run(&raw));
    from_env
        .or_else(|| configured.filter(|value| *value > 0))
        .unwrap_or(DEFAULT_MAX_SPAWNS_PER_RUN)
}

/// pi `normalizeMaxSubagentSpawnsPerRun` (`shared/types.ts:2809-2812`) over the string form: a
/// non-negative integer, kept only when strictly positive.
///
/// `[CYRUP-DELTA]` — the coercion is narrower than upstream's. `normalizeNonNegativeInteger`
/// (`types.ts:2758-2762`) is `Number(value)` guarded by `Number.isInteger`, so it accepts every
/// spelling `Number` accepts of an integral value: `"5.0"`, `"0x10"`, `" 5 "`, `"1e2"`. This is
/// `u32::from_str` on the trimmed string, so only decimal digits parse. Immaterial for an
/// operator-set integer — the two agree on every plain decimal, and a rejected spelling falls
/// through to the config rung and then the default rather than misreading a number — but it is a
/// divergence and belongs beside the module's other three.
fn normalize_max_spawns_per_run(raw: &str) -> Option<u32> {
    raw.trim().parse::<u32>().ok().filter(|value| *value > 0)
}

/// pi `safeRootRunId` (`run-fanout-budget.ts:40-42`): everything outside `[A-Za-z0-9._-]` becomes
/// `_`, truncated to 120 characters, and an empty result falls back to a fresh UUID.
///
/// This is a DIRECTORY-NAME sanitizer, not a validator: the descriptor keeps the caller's raw
/// `rootRunId` (the manifest is compared against that), and only the path segment is scrubbed —
/// which is what keeps a `../` in a run id out of the path while leaving the identity intact.
fn safe_root_run_id(root_run_id: &str) -> String {
    let scrubbed: String = root_run_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_ROOT_RUN_ID_CHARS)
        .collect();
    if scrubbed.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        scrubbed
    }
}

/// pi `ManifestV1` (`run-fanout-budget.ts:15-20`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestV1 {
    version: u32,
    root_run_id: String,
    limit: u32,
    created_at: i64,
}

/// pi `ClaimV1` (`run-fanout-budget.ts:22-27`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimV1 {
    version: u32,
    claim_id: String,
    path: String,
    claimed_at: i64,
}

/// pi `readManifest` (`run-fanout-budget.ts:54-63`) + `parseManifest` (`:44-52`).
fn read_manifest(directory: &Path) -> Result<ManifestV1, RunFanoutError> {
    let raw = std::fs::read_to_string(directory.join("manifest.json")).map_err(|error| {
        invalid(format!(
            "Run fan-out budget manifest is unreadable at '{}': {error}",
            directory.display()
        ))
    })?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
        invalid(format!(
            "Run fan-out budget manifest is unreadable at '{}': {error}",
            directory.display()
        ))
    })?;
    let manifest = serde_json::from_value::<ManifestV1>(parsed)
        .ok()
        .filter(|manifest| {
            manifest.version == 1 && !manifest.root_run_id.is_empty() && manifest.limit > 0
        });
    manifest.ok_or_else(|| {
        invalid(format!(
            "Run fan-out budget manifest is invalid at '{}'.",
            directory.display()
        ))
    })
}

/// pi `validateDirectory` (`run-fanout-budget.ts:65-77`): the descriptor's directory must
/// canonicalize to `root` itself or to something underneath it.
///
/// The canonicalization is the security step, not the string comparison — a descriptor is an
/// operator-supplied (or child-supplied) value arriving over an env var, and a symlink pointing
/// out of the managed root is exactly what this refuses.
fn validate_directory(root: &Path, directory: &Path) -> Result<PathBuf, RunFanoutError> {
    let unavailable = |error: std::io::Error| {
        invalid(format!(
            "Run fan-out budget directory is unavailable: {error}"
        ))
    };
    let real_directory = std::fs::canonicalize(directory).map_err(unavailable)?;
    let real_root = std::fs::canonicalize(root).map_err(unavailable)?;
    if real_directory != real_root && !real_directory.starts_with(&real_root) {
        return Err(invalid(
            "Run fan-out budget directory resolves outside the managed budget root.",
        ));
    }
    Ok(real_directory)
}

/// pi `createRunFanoutBudget` (`run-fanout-budget.ts:79-89`) against the process's real
/// [`run_fanout_root`].
///
/// # Errors
/// [`RunFanoutError::Invalid`] for a non-positive limit (upstream's verbatim sentence) or any
/// filesystem failure creating the ledger.
pub fn create_run_fanout_budget(
    root_run_id: &str,
    limit: u32,
) -> Result<RunFanoutBudgetDescriptor, RunFanoutError> {
    create_run_fanout_budget_in(&run_fanout_root(), root_run_id, limit)
}

/// The rooted form of [`create_run_fanout_budget`].
///
/// # Errors
/// As [`create_run_fanout_budget`].
pub fn create_run_fanout_budget_in(
    root: &Path,
    root_run_id: &str,
    limit: u32,
) -> Result<RunFanoutBudgetDescriptor, RunFanoutError> {
    if limit == 0 {
        return Err(invalid("Run fan-out limit must be a positive integer."));
    }
    create_dir_all_private(root)
        .map_err(|error| invalid(format!("Run fan-out budget root is unavailable: {error}")))?;
    // pi's `do … while (fs.existsSync(directory))`: the UUID makes a collision impossible in
    // practice, and the loop makes it impossible in fact.
    let directory = loop {
        let candidate = root.join(format!(
            "{}-{}",
            safe_root_run_id(root_run_id),
            uuid::Uuid::new_v4()
        ));
        if !candidate.exists() {
            break candidate;
        }
    };
    create_dir_all_private(&directory.join("claims")).map_err(|error| {
        invalid(format!(
            "Run fan-out budget directory is unavailable: {error}"
        ))
    })?;
    let manifest = ManifestV1 {
        version: 1,
        root_run_id: root_run_id.to_string(),
        limit,
        created_at: crate::time::now_epoch_millis(),
    };
    let encoded = serde_json::to_string(&manifest).map_err(|error| {
        invalid(format!(
            "Run fan-out budget manifest is unwritable: {error}"
        ))
    })?;
    // `flag: "wx"` — exclusive create, so a manifest already at that path fails the run rather
    // than being silently replaced with a different limit.
    write_new_private_file(&directory.join("manifest.json"), &format!("{encoded}\n")).map_err(
        |error| {
            invalid(format!(
                "Run fan-out budget manifest is unwritable: {error}"
            ))
        },
    )?;
    Ok(RunFanoutBudgetDescriptor {
        version: 1,
        root_run_id: root_run_id.to_string(),
        directory,
        limit,
        parent_path: None,
    })
}

/// pi `validateRunFanoutBudgetDescriptor` (`run-fanout-budget.ts:91-106`): shape, then the
/// managed-root containment check, then the manifest cross-check.
///
/// Returns the descriptor with its directory CANONICALIZED, which is what every later filesystem
/// operation uses — the caller's own spelling of the path is never re-used after this point.
///
/// # Errors
/// Upstream's three verbatim refusals plus everything [`read_manifest`] raises.
pub fn validate_run_fanout_budget_descriptor(
    descriptor: &RunFanoutBudgetDescriptor,
) -> Result<RunFanoutBudgetDescriptor, RunFanoutError> {
    validate_run_fanout_budget_descriptor_in(&run_fanout_root(), descriptor)
}

/// The rooted form of [`validate_run_fanout_budget_descriptor`].
///
/// # Errors
/// As [`validate_run_fanout_budget_descriptor`].
pub fn validate_run_fanout_budget_descriptor_in(
    root: &Path,
    descriptor: &RunFanoutBudgetDescriptor,
) -> Result<RunFanoutBudgetDescriptor, RunFanoutError> {
    if descriptor.version != 1
        || descriptor.root_run_id.is_empty()
        || descriptor.directory.as_os_str().is_empty()
        || descriptor.limit == 0
    {
        return Err(invalid("Run fan-out budget descriptor is invalid."));
    }
    let directory = validate_directory(root, &descriptor.directory)?;
    let manifest = read_manifest(&directory)?;
    if manifest.root_run_id != descriptor.root_run_id || manifest.limit != descriptor.limit {
        return Err(invalid(
            "Run fan-out budget descriptor does not match its manifest.",
        ));
    }
    Ok(RunFanoutBudgetDescriptor {
        version: 1,
        root_run_id: descriptor.root_run_id.clone(),
        directory,
        limit: descriptor.limit,
        parent_path: descriptor
            .parent_path
            .clone()
            .filter(|value| !value.is_empty()),
    })
}

/// pi `writeRunFanoutBudgetDescriptor` (`run-fanout-budget.ts:108-112`) — persist the descriptor
/// beside an async run so a later status read can report its usage without the env var.
///
/// # Errors
/// As [`validate_run_fanout_budget_descriptor_in`], plus any filesystem failure.
pub fn write_run_fanout_budget_descriptor_in(
    root: &Path,
    async_dir: &Path,
    descriptor: &RunFanoutBudgetDescriptor,
) -> Result<(), RunFanoutError> {
    let valid = validate_run_fanout_budget_descriptor_in(root, descriptor)?;
    std::fs::create_dir_all(async_dir).map_err(|error| {
        invalid(format!(
            "Run fan-out budget descriptor is unwritable at '{}': {error}",
            async_dir.display()
        ))
    })?;
    let encoded = serde_json::to_string(&valid).map_err(|error| {
        invalid(format!(
            "Run fan-out budget descriptor is unwritable: {error}"
        ))
    })?;
    write_private_file(
        &async_dir.join("run-fanout-budget.json"),
        &format!("{encoded}\n"),
    )
    .map_err(|error| {
        invalid(format!(
            "Run fan-out budget descriptor is unwritable at '{}': {error}",
            async_dir.display()
        ))
    })
}

/// pi `readRunFanoutBudgetDescriptor` (`run-fanout-budget.ts:114-123`).
///
/// A MISSING file is `Ok(None)` — an async run that predates the ledger, or one that never had a
/// budget. A PRESENT but unreadable/invalid file is an error, because a run whose ledger cannot be
/// read must not silently continue as if it were unbounded.
///
/// # Errors
/// Upstream's verbatim `Invalid persisted run fan-out budget '<path>': <cause>`.
pub fn read_run_fanout_budget_descriptor_in(
    root: &Path,
    async_dir: Option<&Path>,
) -> Result<Option<RunFanoutBudgetDescriptor>, RunFanoutError> {
    let Some(async_dir) = async_dir else {
        return Ok(None);
    };
    let descriptor_path = async_dir.join("run-fanout-budget.json");
    if !descriptor_path.exists() {
        return Ok(None);
    }
    let context = |cause: String| {
        invalid(format!(
            "Invalid persisted run fan-out budget '{}': {cause}",
            descriptor_path.display()
        ))
    };
    let raw =
        std::fs::read_to_string(&descriptor_path).map_err(|error| context(error.to_string()))?;
    let parsed = serde_json::from_str::<RunFanoutBudgetDescriptor>(&raw)
        .map_err(|error| context(error.to_string()))?;
    validate_run_fanout_budget_descriptor_in(root, &parsed)
        .map(Some)
        .map_err(|error| context(error.to_string()))
}

/// pi `encodeRunFanoutBudgetDescriptor` (`run-fanout-budget.ts:125-127`) — base64url JSON, the
/// form that crosses the spawn boundary in [`RUN_FANOUT_BUDGET_ENV`].
///
/// # Errors
/// As [`validate_run_fanout_budget_descriptor_in`]: an unvalidatable descriptor is never encoded,
/// so a child cannot be handed a ledger this process could not itself open.
pub fn encode_run_fanout_budget_descriptor_in(
    root: &Path,
    descriptor: &RunFanoutBudgetDescriptor,
) -> Result<String, RunFanoutError> {
    use base64::Engine as _;
    let valid = validate_run_fanout_budget_descriptor_in(root, descriptor)?;
    let json = serde_json::to_vec(&valid).map_err(|error| {
        invalid(format!(
            "Run fan-out budget descriptor is unencodable: {error}"
        ))
    })?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
}

/// pi `decodeRunFanoutBudgetDescriptor` (`run-fanout-budget.ts:129-136`).
///
/// An ABSENT or empty value is `Ok(None)` ("this process did not inherit a budget"); a present but
/// undecodable one is an error, never a silent `None` — the fail-open arm is not expressible, for
/// the same reason `CFG-080` made the tool-budget decode refuse rather than drop.
///
/// # Errors
/// Upstream's verbatim `Invalid inherited run fan-out budget: <cause>`.
pub fn decode_run_fanout_budget_descriptor_in(
    root: &Path,
    encoded: Option<&str>,
) -> Result<Option<RunFanoutBudgetDescriptor>, RunFanoutError> {
    use base64::Engine as _;
    let Some(encoded) = encoded.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let context = |cause: String| invalid(format!("Invalid inherited run fan-out budget: {cause}"));
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        // Node's `Buffer.from(v, "base64url")` accepts a padded value too.
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(encoded))
        .map_err(|error| context(error.to_string()))?;
    let parsed = serde_json::from_slice::<RunFanoutBudgetDescriptor>(&bytes)
        .map_err(|error| context(error.to_string()))?;
    validate_run_fanout_budget_descriptor_in(root, &parsed)
        .map(Some)
        .map_err(|error| context(error.to_string()))
}

/// pi `claimCount` (`run-fanout-budget.ts:200-207`): the number of `NNNNNN.json` slot files. Only
/// that exact six-digit shape counts, so an unrelated file dropped into `claims/` neither inflates
/// nor deflates the ledger.
fn claim_count(directory: &Path) -> Result<u32, RunFanoutError> {
    let claims_dir = directory.join("claims");
    let entries = std::fs::read_dir(&claims_dir).map_err(|error| {
        invalid(format!(
            "Run fan-out claims directory is unreadable at '{}': {error}",
            claims_dir.display()
        ))
    })?;
    let mut count: u32 = 0;
    for entry in entries {
        let entry = entry.map_err(|error| {
            invalid(format!(
                "Run fan-out claims directory is unreadable at '{}': {error}",
                claims_dir.display()
            ))
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if is_claim_slot_name(name) {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

/// pi's `/^\d{6}\.json$/` (`run-fanout-budget.ts:206`).
fn is_claim_slot_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    stem.len() == 6 && stem.bytes().all(|b| b.is_ascii_digit())
}

/// pi `getRunFanoutBudgetSnapshot` (`run-fanout-budget.ts:209-213`).
///
/// # Errors
/// As [`validate_run_fanout_budget_descriptor_in`] and [`claim_count`].
pub fn run_fanout_budget_snapshot_in(
    root: &Path,
    descriptor: &RunFanoutBudgetDescriptor,
) -> Result<RunFanoutBudgetSnapshot, RunFanoutError> {
    let valid = validate_run_fanout_budget_descriptor_in(root, descriptor)?;
    let used = claim_count(&valid.directory)?;
    Ok(RunFanoutBudgetSnapshot {
        used,
        limit: valid.limit,
        remaining: valid.limit.saturating_sub(used),
    })
}

/// pi `qualifyRunFanoutPaths` (`run-fanout-budget.ts:215-218`).
fn qualify_run_fanout_paths(
    descriptor: &RunFanoutBudgetDescriptor,
    paths: &[String],
) -> Vec<String> {
    let prefix = descriptor
        .parent_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    paths
        .iter()
        .map(|item| match prefix {
            Some(prefix) => format!("{prefix}/{item}"),
            None => item.clone(),
        })
        .collect()
}

/// pi `formatRunFanoutBudget` (`run-fanout-budget.ts:274-276`).
#[must_use]
pub fn format_run_fanout_budget(snapshot: &RunFanoutBudgetSnapshot) -> String {
    format!(
        "Run fan-out: {}/{} used, {} remaining",
        snapshot.used, snapshot.limit, snapshot.remaining
    )
}

/// pi `formatRunFanoutRejection` (`run-fanout-budget.ts:278-280`), verbatim — including the
/// closing sentence that names the config key an operator would raise.
#[must_use]
pub fn format_run_fanout_rejection(rejection: &RunFanoutRejection) -> String {
    format!(
        "Run fan-out limit reached at {} ({}/{} used; {} requested, {} remaining). No children \
         from this admission group were started. Start a new top-level run or raise \
         config.maxSubagentSpawnsPerRun.",
        rejection.path, rejection.used, rejection.limit, rejection.requested, rejection.remaining
    )
}

/// Which rung of pi's `env -> config -> default` ladder supplied the configured per-run limit
/// (`extension/doctor.ts:190-192` @v0.64.0 computes exactly this to label the doctor line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxSpawnsPerRunSource {
    /// [`MAX_SPAWNS_PER_RUN_ENV`] (or its `PI_` alias) supplied a usable value.
    Environment,
    /// The extension config supplied a usable value.
    Config,
    /// Neither did, so [`DEFAULT_MAX_SPAWNS_PER_RUN`] applies.
    Default,
}

impl MaxSpawnsPerRunSource {
    /// pi's own words for the three rungs (`extension/doctor.ts:190-192`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Config => "config",
            Self::Default => "default",
        }
    }

    /// Which rung [`resolve_max_spawns_per_run_with`] took, over the same injected environment.
    #[must_use]
    pub fn resolve_with(get: &dyn Fn(&str) -> Option<String>, configured: Option<u32>) -> Self {
        if get(MAX_SPAWNS_PER_RUN_ENV)
            .or_else(|| get(MAX_SPAWNS_PER_RUN_ENV_PI_ALIAS))
            .and_then(|raw| normalize_max_spawns_per_run(&raw))
            .is_some()
        {
            Self::Environment
        } else if configured.is_some_and(|value| value > 0) {
            Self::Config
        } else {
            Self::Default
        }
    }
}

/// pi `formatRunFanoutSection` (`extension/doctor.ts:180-193` @v0.64.0) as a VALUE: the three
/// mutually exclusive states the doctor's "Run fan-out budget" block can be in, resolved once so
/// the report itself does no environment reads.
///
/// This is the surface that makes both of this row's env vars observable to an operator —
/// [`RUN_FANOUT_BUDGET_ENV`] through [`Self::Inherited`], [`MAX_SPAWNS_PER_RUN_ENV`] through
/// [`Self::Configured`]'s source label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunFanoutDoctor {
    /// This process inherited a ledger from its parent; the snapshot is its live claim count.
    Inherited {
        /// The ledger's `rootRunId`.
        root_run_id: String,
        /// Claims recorded against it right now.
        snapshot: RunFanoutBudgetSnapshot,
    },
    /// [`RUN_FANOUT_BUDGET_ENV`] was set but did not decode (or its ledger is unreadable). pi
    /// renders the cause rather than falling back to the configured line, so a broken inheritance
    /// is never mistaken for a top-level run.
    InvalidInherited(String),
    /// No inherited ledger: the limit a run started here would be created with.
    Configured {
        /// The resolved limit.
        limit: u32,
        /// Which rung supplied it.
        source: MaxSpawnsPerRunSource,
    },
}

impl RunFanoutDoctor {
    /// Resolve the block against the process environment and the real [`run_fanout_root`].
    #[must_use]
    pub fn resolve(configured: Option<u32>) -> Self {
        Self::resolve_with(
            &|key| std::env::var(key).ok(),
            &run_fanout_root(),
            configured,
        )
    }

    /// The pure core of [`Self::resolve`], with the environment and the managed root injected.
    #[must_use]
    pub fn resolve_with(
        get: &dyn Fn(&str) -> Option<String>,
        root: &Path,
        configured: Option<u32>,
    ) -> Self {
        let inherited = get(RUN_FANOUT_BUDGET_ENV).or_else(|| get(RUN_FANOUT_BUDGET_ENV_PI_ALIAS));
        match decode_run_fanout_budget_descriptor_in(root, inherited.as_deref()) {
            Ok(Some(descriptor)) => match run_fanout_budget_snapshot_in(root, &descriptor) {
                Ok(snapshot) => Self::Inherited {
                    root_run_id: descriptor.root_run_id,
                    snapshot,
                },
                Err(error) => Self::InvalidInherited(error.to_string()),
            },
            Ok(None) => Self::Configured {
                limit: resolve_max_spawns_per_run_with(get, configured),
                source: MaxSpawnsPerRunSource::resolve_with(get, configured),
            },
            Err(error) => Self::InvalidInherited(error.to_string()),
        }
    }

    /// The block's lines, from `extension/doctor.ts:181-193`, plus one `[CYRUP-DELTA]` line the
    /// operator is owed — see `ENFORCEMENT` below. The shared closing "reset boundary" line says
    /// in as many words that claims are cumulative.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        const RESET_BOUNDARY: &str = "- reset boundary: cumulative claims are never released; \
             a new top-level run creates a new budget";
        // `[CYRUP-DELTA]` — not upstream's, and it stays until the enforcement slice lands.
        // CFG-067 ported the ledger and this doctor surface; nothing in cyrup CREATES a budget,
        // writes `CYRUP_SUBAGENT_RUN_FANOUT_BUDGET` into a fan-out child's environment, or CLAIMS
        // against one (pi does all three: `runs/shared/pi-args.ts:942-943`,
        // `runs/foreground/subagent-executor.ts:6473-6477`). Without this line the block reports a
        // cap that no run applies and a usage figure that can never leave 0 — precisely the
        // "misleading doctor surface" CFG-067's own 2026-09-04 disposition refused to ship the
        // resolved number ahead of. Disclosed here as well as in the ledger, because the ledger is
        // not what `/subagents-doctor` shows an operator.
        const ENFORCEMENT: &str = "- enforcement: NOT WIRED — no run creates or claims against \
             this budget yet, so the limit above is reported but not applied";
        match self {
            Self::Inherited {
                root_run_id,
                snapshot,
            } => {
                // pi renders `formatRunFanoutBudget(...)` with its own `Run fan-out: ` prefix
                // stripped, because the section header already says it (`:184`). If the formatter's
                // prefix ever changes, degrade to the whole sentence rather than to nothing.
                let formatted = format_run_fanout_budget(snapshot);
                let usage = formatted
                    .strip_prefix("Run fan-out: ")
                    .unwrap_or(formatted.as_str());
                vec![
                    format!("- usage: {usage}"),
                    format!("- root run: {root_run_id}"),
                    ENFORCEMENT.to_string(),
                    RESET_BOUNDARY.to_string(),
                ]
            }
            Self::InvalidInherited(cause) => {
                vec![format!("- inherited budget: invalid — {cause}")]
            }
            Self::Configured { limit, source } => vec![
                format!("- configured limit: {limit} ({})", source.label()),
                "- usage: available after a run starts".to_string(),
                ENFORCEMENT.to_string(),
                RESET_BOUNDARY.to_string(),
            ],
        }
    }
}

/// pi `claimRunFanoutBatch` (`run-fanout-budget.ts:266-268`), blocking.
///
/// This function BLOCKS the calling thread while it waits on the admission lock (up to the sum of
/// [`FILE_SYSTEM_RETRY_DELAYS_MS`], ~7.9s). An async caller must reach it through
/// `tokio::task::spawn_blocking`.
///
/// # Errors
/// [`RunFanoutError::Limit`] when the batch does not fit — nothing is claimed — and
/// [`RunFanoutError::Invalid`] for everything else.
pub fn claim_run_fanout_batch_in(
    root: &Path,
    descriptor: &RunFanoutBudgetDescriptor,
    paths: &[String],
) -> Result<RunFanoutBudgetSnapshot, RunFanoutError> {
    claim_run_fanout_batch_with_commit_in(root, descriptor, paths, &blocking_wait, |snapshot| {
        *snapshot
    })
}

/// pi `claimRunFanoutBatchWithCommit` (`run-fanout-budget.ts:270-272`) — run `commit` while the
/// claims are held and the admission lock is still ours, so a caller that persists its own state
/// cannot have another process admit between the claim and the persist.
///
/// Blocking, as [`claim_run_fanout_batch_in`]. `wait` is the parking strategy for lock contention
/// ([`blocking_wait`] is upstream's).
///
/// # Errors
/// As [`claim_run_fanout_batch_in`]. When `commit` is not reached — including when it panics and
/// unwinds — the claims this call created are removed again, so a failed admission group leaves the
/// ledger exactly as it found it. A `commit` that signals its own failure through its return value
/// (`T = Result<_, _>`) is a SUCCESS to this function and keeps its claims, which is upstream's
/// behaviour too: only a throw reaches its `catch` (`run-fanout-budget.ts:257-262` @v0.64.0).
pub fn claim_run_fanout_batch_with_commit_in<T>(
    root: &Path,
    descriptor: &RunFanoutBudgetDescriptor,
    paths: &[String],
    wait: &dyn Fn(Duration),
    commit: impl FnOnce(&RunFanoutBudgetSnapshot) -> T,
) -> Result<T, RunFanoutError> {
    let valid = validate_run_fanout_budget_descriptor_in(root, descriptor)?;
    if paths.is_empty() {
        // pi commits against the CURRENT snapshot without taking the lock: an empty admission
        // group changes nothing, so there is nothing to serialize against.
        let snapshot = run_fanout_budget_snapshot_in(root, &valid)?;
        return Ok(commit(&snapshot));
    }
    let qualified = qualify_run_fanout_paths(&valid, paths);
    with_admission_lock(&valid.directory, wait, || {
        let before = run_fanout_budget_snapshot_in(root, &valid)?;
        let requested = u32::try_from(qualified.len()).unwrap_or(u32::MAX);
        if requested > before.remaining {
            let path = qualified
                .get(before.remaining as usize)
                .or_else(|| qualified.first())
                .cloned()
                .unwrap_or_default();
            return Err(RunFanoutError::Limit(Box::new(RunFanoutRejection {
                code: RunFanoutRejection::CODE.to_string(),
                path,
                requested,
                used: before.used,
                limit: before.limit,
                remaining: before.remaining,
            })));
        }
        // Upstream wraps slot creation AND the `commit` call in ONE `try` whose `catch` unlinks
        // every slot in `created` before rethrowing (`run-fanout-budget.ts:238-262` @v0.64.0, the
        // unwind at `:257-262`). An `if outcome.is_err()` rollback is not that: a panic unwinding
        // out of the caller-supplied `commit` walks straight past it, and claims are permanent by
        // this module's own reset boundary, so the run's cap would be burned for good. The same
        // drop-guard shape the admission lock uses below gives upstream's guarantee under unwind;
        // `disarm` is the success path, standing in for falling off the end of upstream's `try`.
        struct RollbackOnDrop {
            created: Vec<PathBuf>,
            armed: bool,
        }
        impl RollbackOnDrop {
            fn disarm(&mut self) {
                self.armed = false;
            }
        }
        impl Drop for RollbackOnDrop {
            fn drop(&mut self) {
                if !self.armed {
                    return;
                }
                for slot_path in &self.created {
                    let _ = std::fs::remove_file(slot_path);
                }
            }
        }
        let mut rollback = RollbackOnDrop {
            created: Vec::new(),
            armed: true,
        };
        let outcome = (|| -> Result<T, RunFanoutError> {
            for claim_path in &qualified {
                let slot = take_free_slot(&valid, claim_path, &mut rollback.created)?;
                debug_assert!(slot, "a free slot must exist below the limit");
            }
            let after = run_fanout_budget_snapshot_in(root, &valid)?;
            Ok(commit(&after))
        })();
        if outcome.is_ok() {
            rollback.disarm();
        }
        outcome
    })
}

/// One iteration of pi's inner `for (let slot = 0; slot < valid.limit; slot++)` loop
/// (`run-fanout-budget.ts:238-254`): find the lowest slot number whose file does not yet exist and
/// create it EXCLUSIVELY. `EEXIST` means another process (or an earlier path in this batch) owns
/// that slot, so the search continues; every other error aborts the batch.
fn take_free_slot(
    descriptor: &RunFanoutBudgetDescriptor,
    claim_path: &str,
    created: &mut Vec<PathBuf>,
) -> Result<bool, RunFanoutError> {
    for slot in 0..descriptor.limit {
        let slot_path = descriptor
            .directory
            .join("claims")
            .join(format!("{slot:06}.json"));
        let claim = ClaimV1 {
            version: 1,
            claim_id: uuid::Uuid::new_v4().to_string(),
            path: claim_path.to_string(),
            claimed_at: crate::time::now_epoch_millis(),
        };
        let encoded = serde_json::to_string(&claim)
            .map_err(|error| invalid(format!("Run fan-out claim is unwritable: {error}")))?;
        match write_new_private_file(&slot_path, &format!("{encoded}\n")) {
            Ok(()) => {
                created.push(slot_path);
                return Ok(true);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(invalid(format!(
                    "Run fan-out claim is unwritable at '{}': {error}",
                    slot_path.display()
                )));
            }
        }
    }
    Ok(false)
}

/// pi `AdmissionLockOwner` (`run-fanout-budget.ts:140-143`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AdmissionLockOwner {
    pid: i32,
    token: String,
}

/// pi `readAdmissionLockOwner` (`run-fanout-budget.ts:148-154`).
fn read_admission_lock_owner(lock_path: &Path) -> Option<AdmissionLockOwner> {
    let raw = std::fs::read_to_string(lock_path.join("owner.json")).ok()?;
    let owner = serde_json::from_str::<AdmissionLockOwner>(&raw).ok()?;
    (owner.pid > 0 && !owner.token.is_empty()).then_some(owner)
}

/// pi `admissionLockIsStale` (`run-fanout-budget.ts:156-166`).
///
/// A lock whose owner PID is still alive is NEVER stale, however old it is — that is what stops a
/// slow-but-live admission from having its lock stolen. Only when the owner is gone (or the owner
/// file is unreadable and the directory's mtime is older than [`ADMISSION_LOCK_STALE_MS`]) may the
/// lock be reclaimed. `EPERM` from the liveness probe means a live process owned by another user,
/// so it counts as alive.
fn admission_lock_is_stale(lock_path: &Path) -> Result<bool, RunFanoutError> {
    if let Some(owner) = read_admission_lock_owner(lock_path) {
        return Ok(!process_is_alive(owner.pid));
    }
    match std::fs::metadata(lock_path).and_then(|meta| meta.modified()) {
        Ok(modified) => Ok(modified
            .elapsed()
            .is_ok_and(|age| age > Duration::from_millis(ADMISSION_LOCK_STALE_MS))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(invalid(format!(
            "Run fan-out admission lock is unreadable at '{}': {error}",
            lock_path.display()
        ))),
    }
}

/// pi's `process.kill(pid, 0)` liveness probe (`run-fanout-budget.ts:158-163`).
///
/// `[CYRUP-DELTA]` — `nix::sys::signal::kill(pid, None)`, because this crate is
/// `#![forbid(unsafe_code)]`. `EPERM` is "alive but not ours", exactly as upstream's
/// `code !== "EPERM"` test decides.
fn process_is_alive(pid: i32) -> bool {
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(nix::errno::Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// pi's default `waitForFileSystemRetry` (`shared/file-system-retry.ts`), which parks the calling
/// thread rather than spinning.
pub fn blocking_wait(delay: Duration) {
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
}

/// pi `withAdmissionLock` (`run-fanout-budget.ts:172-205`): an exclusive `mkdir` as the lock, an
/// `owner.json` naming the holder, a bounded retry ladder, stale-owner reclamation, and a release
/// that fires on every exit path and only when the lock is still ours.
///
/// The token check on release (`:203`) is what keeps a process that lost its lock to a staleness
/// reclaim from deleting the NEW owner's lock on its way out. The release itself is a drop guard
/// because upstream's is a `finally` (`:202-204`) — see the comment at the call.
fn with_admission_lock<T>(
    directory: &Path,
    wait: &dyn Fn(Duration),
    operation: impl FnOnce() -> Result<T, RunFanoutError>,
) -> Result<T, RunFanoutError> {
    let lock_path = directory.join("admission.lock");
    let owner = AdmissionLockOwner {
        pid: std::process::id().try_into().unwrap_or(i32::MAX),
        token: uuid::Uuid::new_v4().to_string(),
    };
    let mut attempt = 0usize;
    loop {
        match create_private_dir(&lock_path) {
            Ok(()) => {
                let encoded = serde_json::to_string(&owner).map_err(|error| {
                    invalid(format!("Run fan-out admission lock is unwritable: {error}"))
                })?;
                if let Err(error) = write_private_file(&lock_path.join("owner.json"), &encoded) {
                    let _ = std::fs::remove_dir_all(&lock_path);
                    return Err(invalid(format!(
                        "Run fan-out admission lock is unwritable at '{}': {error}",
                        lock_path.display()
                    )));
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if admission_lock_is_stale(&lock_path)? {
                    let stale_path =
                        directory.join(format!("admission.stale-{}", uuid::Uuid::new_v4()));
                    match std::fs::rename(&lock_path, &stale_path) {
                        Ok(()) => {
                            let _ = std::fs::remove_dir_all(&stale_path);
                            continue;
                        }
                        Err(reclaim) if reclaim.kind() == std::io::ErrorKind::NotFound => {}
                        Err(reclaim) => {
                            return Err(invalid(format!(
                                "Run fan-out admission lock is unreclaimable at '{}': {reclaim}",
                                lock_path.display()
                            )));
                        }
                    }
                }
                let Some(delay) = FILE_SYSTEM_RETRY_DELAYS_MS.get(attempt) else {
                    return Err(invalid(format!(
                        "Timed out acquiring run fan-out admission lock at '{}'.",
                        directory.display()
                    )));
                };
                attempt = attempt.saturating_add(1);
                wait(Duration::from_millis(*delay));
            }
            Err(error) => {
                return Err(invalid(format!(
                    "Run fan-out admission lock is unavailable at '{}': {error}",
                    lock_path.display()
                )));
            }
        }
    }
    // The release is a drop guard, not a straight-line statement, because upstream's is
    // `try { return operation(); } finally { ... }` (`run-fanout-budget.ts:201-204`) — it fires on
    // the abrupt path too. `operation` reaches the caller-supplied `commit` closure of
    // `claim_run_fanout_batch_with_commit_in`, so a panic there would unwind past a plain release
    // and leave `admission.lock` on disk owned by a LIVE pid. `admission_lock_is_stale` never
    // reclaims a live owner's lock, so every later admission for that run would fail with
    // "Timed out acquiring run fan-out admission lock" until the process exits.
    struct ReleaseOnDrop<'a> {
        lock_path: &'a Path,
        token: &'a str,
    }
    impl Drop for ReleaseOnDrop<'_> {
        fn drop(&mut self) {
            // The token check (`:203`) keeps a holder that lost its lock to a staleness reclaim
            // from deleting the NEW owner's lock on its way out.
            if read_admission_lock_owner(self.lock_path)
                .is_some_and(|held| held.token == self.token)
            {
                let _ = std::fs::remove_dir_all(self.lock_path);
            }
        }
    }
    let _release = ReleaseOnDrop {
        lock_path: &lock_path,
        token: &owner.token,
    };
    operation()
}

/// `fs.mkdirSync(path, { recursive: true, mode: 0o700 })`.
fn create_dir_all_private(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// `fs.mkdirSync(path, { mode: 0o700 })` — NOT recursive, so an existing directory is `EEXIST`.
/// That is the lock primitive itself, so the non-recursive form is load-bearing.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// `fs.writeFileSync(path, contents, { mode: 0o600 })` — truncating.
fn write_private_file(file: &Path, contents: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut handle = options.open(file)?;
    handle.write_all(contents.as_bytes())?;
    handle.flush()
}

/// `fs.openSync(path, "wx", 0o600)` — EXCLUSIVE create; `AlreadyExists` is a meaningful outcome
/// for both callers (a taken claim slot, a manifest that already exists).
fn write_new_private_file(file: &Path, contents: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut handle = options.open(file)?;
    handle.write_all(contents.as_bytes())?;
    handle.flush()
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

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    fn no_wait(_: Duration) {}

    #[test]
    fn the_env_names_are_the_cyrup_spelling_with_the_pi_one_as_a_read_side_alias() {
        assert_eq!(RUN_FANOUT_BUDGET_ENV, "CYRUP_SUBAGENT_RUN_FANOUT_BUDGET");
        assert_eq!(
            RUN_FANOUT_BUDGET_ENV_PI_ALIAS,
            "PI_SUBAGENT_RUN_FANOUT_BUDGET"
        );
        assert_eq!(MAX_SPAWNS_PER_RUN_ENV, "CYRUP_SUBAGENT_MAX_SPAWNS_PER_RUN");
        assert_eq!(
            MAX_SPAWNS_PER_RUN_ENV_PI_ALIAS,
            "PI_SUBAGENT_MAX_SPAWNS_PER_RUN"
        );
        assert_eq!(DEFAULT_MAX_SPAWNS_PER_RUN, 64);
    }

    #[test]
    fn the_limit_resolver_follows_pis_env_then_config_then_default_ladder() {
        let none = |_: &str| None;
        assert_eq!(resolve_max_spawns_per_run_with(&none, None), 64);
        assert_eq!(resolve_max_spawns_per_run_with(&none, Some(7)), 7);
        let env = |key: &str| (key == MAX_SPAWNS_PER_RUN_ENV).then(|| "3".to_string());
        assert_eq!(resolve_max_spawns_per_run_with(&env, Some(7)), 3);
    }

    #[test]
    fn zero_is_not_unlimited_on_either_surface_unlike_the_per_session_cap() {
        // pi `normalizeMaxSubagentSpawnsPerRun` drops anything not `> 0`, so `0` falls THROUGH.
        let zero_env = |key: &str| (key == MAX_SPAWNS_PER_RUN_ENV).then(|| "0".to_string());
        assert_eq!(resolve_max_spawns_per_run_with(&zero_env, Some(9)), 9);
        assert_eq!(resolve_max_spawns_per_run_with(&zero_env, None), 64);
        let none = |_: &str| None;
        assert_eq!(resolve_max_spawns_per_run_with(&none, Some(0)), 64);
    }

    #[test]
    fn a_typo_in_the_env_falls_through_rather_than_disabling_the_cap() {
        let junk = |key: &str| (key == MAX_SPAWNS_PER_RUN_ENV).then(|| "not-a-number".to_string());
        assert_eq!(resolve_max_spawns_per_run_with(&junk, Some(5)), 5);
    }

    #[test]
    fn the_pi_alias_is_consulted_only_when_the_cyrup_spelling_is_unset() {
        let alias_only =
            |key: &str| (key == MAX_SPAWNS_PER_RUN_ENV_PI_ALIAS).then(|| "11".to_string());
        assert_eq!(resolve_max_spawns_per_run_with(&alias_only, None), 11);
        let both = |key: &str| match key {
            MAX_SPAWNS_PER_RUN_ENV => Some("2".to_string()),
            MAX_SPAWNS_PER_RUN_ENV_PI_ALIAS => Some("11".to_string()),
            _ => None,
        };
        assert_eq!(resolve_max_spawns_per_run_with(&both, None), 2);
    }

    #[test]
    fn a_created_budget_starts_empty_and_records_its_manifest() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 3).unwrap();
        assert_eq!(descriptor.limit, 3);
        assert_eq!(descriptor.root_run_id, "run-1");
        assert!(descriptor.directory.starts_with(root.path()));
        let snapshot = run_fanout_budget_snapshot_in(root.path(), &descriptor).unwrap();
        assert_eq!(
            snapshot,
            RunFanoutBudgetSnapshot {
                used: 0,
                limit: 3,
                remaining: 3
            }
        );
    }

    #[test]
    fn a_zero_limit_is_refused_with_upstreams_sentence() {
        let root = tempfile::tempdir().unwrap();
        let error = create_run_fanout_budget_in(root.path(), "run-1", 0).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Run fan-out limit must be a positive integer."
        );
    }

    #[test]
    fn a_path_separator_in_the_run_id_never_reaches_the_directory_name() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "../../escape", 2).unwrap();
        // The identity is preserved; only the path segment is scrubbed.
        assert_eq!(descriptor.root_run_id, "../../escape");
        let name = descriptor.directory.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(".._.._escape-"), "got {name}");
        assert!(descriptor.directory.starts_with(root.path()));
    }

    #[test]
    fn claims_accumulate_and_are_never_released() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 4).unwrap();
        let first =
            claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["a", "b"])).unwrap();
        assert_eq!(first.used, 2);
        assert_eq!(first.remaining, 2);
        let second = claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["c"])).unwrap();
        assert_eq!(second.used, 3);
        assert_eq!(second.remaining, 1);
    }

    #[test]
    fn a_batch_that_does_not_fit_is_refused_whole_and_claims_nothing() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 3).unwrap();
        claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["a", "b"])).unwrap();
        let error =
            claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["c", "d"])).unwrap_err();
        let RunFanoutError::Limit(rejection) = &error else {
            panic!("expected a limit refusal, got {error:?}");
        };
        assert_eq!(rejection.code, "RUN_FANOUT_LIMIT");
        // The first path that did NOT fit, i.e. `qualified[remaining]`.
        assert_eq!(rejection.path, "d");
        assert_eq!(rejection.requested, 2);
        assert_eq!(rejection.used, 2);
        assert_eq!(rejection.remaining, 1);
        assert_eq!(
            error.to_string(),
            "Run fan-out limit reached at d (2/3 used; 2 requested, 1 remaining). No children \
             from this admission group were started. Start a new top-level run or raise \
             config.maxSubagentSpawnsPerRun."
        );
        // Nothing from the refused group was recorded.
        let snapshot = run_fanout_budget_snapshot_in(root.path(), &descriptor).unwrap();
        assert_eq!(snapshot.used, 2);
    }

    #[test]
    fn a_batch_that_lands_exactly_on_the_cap_is_admitted() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 2).unwrap();
        let snapshot =
            claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["a", "b"])).unwrap();
        assert_eq!(snapshot.remaining, 0);
        let error =
            claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["c"])).unwrap_err();
        assert!(matches!(error, RunFanoutError::Limit(_)));
    }

    #[test]
    fn an_empty_admission_group_never_touches_the_ledger() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 1).unwrap();
        let snapshot = claim_run_fanout_batch_in(root.path(), &descriptor, &[]).unwrap();
        assert_eq!(snapshot.used, 0);
        assert!(!descriptor.directory.join("admission.lock").exists());
    }

    #[test]
    fn a_parent_path_qualifies_every_claim_in_the_batch() {
        let root = tempfile::tempdir().unwrap();
        let mut descriptor = create_run_fanout_budget_in(root.path(), "run-1", 1).unwrap();
        descriptor.parent_path = Some("workflow[a]".to_string());
        let error =
            claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["chain[0]", "chain[1]"]))
                .unwrap_err();
        let RunFanoutError::Limit(rejection) = &error else {
            panic!("expected a limit refusal");
        };
        assert_eq!(rejection.path, "workflow[a]/chain[1]");
    }

    #[test]
    fn a_commit_closure_runs_while_the_claims_are_held() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 2).unwrap();
        let observed = claim_run_fanout_batch_with_commit_in(
            root.path(),
            &descriptor,
            &paths(&["a"]),
            &no_wait,
            |snapshot| snapshot.used,
        )
        .unwrap();
        assert_eq!(observed, 1);
    }

    #[test]
    fn the_descriptor_round_trips_through_the_env_encoding() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 5).unwrap();
        let encoded = encode_run_fanout_budget_descriptor_in(root.path(), &descriptor).unwrap();
        let decoded = decode_run_fanout_budget_descriptor_in(root.path(), Some(&encoded))
            .unwrap()
            .unwrap();
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn an_absent_inherited_budget_is_none_but_a_corrupt_one_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            decode_run_fanout_budget_descriptor_in(root.path(), None).unwrap(),
            None
        );
        assert_eq!(
            decode_run_fanout_budget_descriptor_in(root.path(), Some("  ")).unwrap(),
            None
        );
        let error = decode_run_fanout_budget_descriptor_in(root.path(), Some("!!!not-base64!!!"))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Invalid inherited run fan-out budget: "),
            "got {error}"
        );
    }

    #[test]
    fn a_descriptor_pointing_outside_the_managed_root_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let stray = create_run_fanout_budget_in(elsewhere.path(), "run-1", 2).unwrap();
        let error = validate_run_fanout_budget_descriptor_in(root.path(), &stray).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Run fan-out budget directory resolves outside the managed budget root."
        );
    }

    #[test]
    fn a_descriptor_whose_limit_disagrees_with_its_manifest_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let mut descriptor = create_run_fanout_budget_in(root.path(), "run-1", 2).unwrap();
        descriptor.limit = 9999;
        let error = validate_run_fanout_budget_descriptor_in(root.path(), &descriptor).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Run fan-out budget descriptor does not match its manifest."
        );
    }

    #[test]
    fn the_persisted_descriptor_is_absent_missing_and_refused_when_corrupt() {
        let root = tempfile::tempdir().unwrap();
        let async_dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_run_fanout_budget_descriptor_in(root.path(), None).unwrap(),
            None
        );
        assert_eq!(
            read_run_fanout_budget_descriptor_in(root.path(), Some(async_dir.path())).unwrap(),
            None
        );
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 2).unwrap();
        write_run_fanout_budget_descriptor_in(root.path(), async_dir.path(), &descriptor).unwrap();
        assert_eq!(
            read_run_fanout_budget_descriptor_in(root.path(), Some(async_dir.path())).unwrap(),
            Some(descriptor)
        );
        std::fs::write(async_dir.path().join("run-fanout-budget.json"), "{").unwrap();
        let error =
            read_run_fanout_budget_descriptor_in(root.path(), Some(async_dir.path())).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Invalid persisted run fan-out budget '"),
            "got {error}"
        );
    }

    #[test]
    fn only_six_digit_slot_files_are_counted_as_claims() {
        assert!(is_claim_slot_name("000000.json"));
        assert!(is_claim_slot_name("123456.json"));
        assert!(!is_claim_slot_name("12345.json"));
        assert!(!is_claim_slot_name("1234567.json"));
        assert!(!is_claim_slot_name("00000a.json"));
        assert!(!is_claim_slot_name("000000.txt"));
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 2).unwrap();
        std::fs::write(descriptor.directory.join("claims").join("notes.txt"), "x").unwrap();
        assert_eq!(
            run_fanout_budget_snapshot_in(root.path(), &descriptor)
                .unwrap()
                .used,
            0
        );
    }

    #[test]
    fn a_live_owners_lock_is_never_stale_and_times_the_waiter_out() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 2).unwrap();
        let lock_path = descriptor.directory.join("admission.lock");
        create_private_dir(&lock_path).unwrap();
        // OUR pid is by definition alive, so this lock can never be reclaimed as stale.
        write_private_file(
            &lock_path.join("owner.json"),
            &serde_json::to_string(&AdmissionLockOwner {
                pid: std::process::id().try_into().unwrap(),
                token: "someone-else".to_string(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(!admission_lock_is_stale(&lock_path).unwrap());
        let error = claim_run_fanout_batch_with_commit_in(
            root.path(),
            &descriptor,
            &paths(&["a"]),
            &no_wait,
            |snapshot| *snapshot,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Timed out acquiring run fan-out admission lock at '"),
            "got {error}"
        );
        // The live owner's lock survives the timed-out contender.
        assert!(lock_path.exists());
    }

    #[test]
    fn a_dead_owners_lock_is_stale_and_is_reclaimed() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 2).unwrap();
        let lock_path = descriptor.directory.join("admission.lock");
        create_private_dir(&lock_path).unwrap();
        // PID 0 is never a live process id for `kill(2)` in the sense this probe needs.
        write_private_file(
            &lock_path.join("owner.json"),
            &serde_json::to_string(&AdmissionLockOwner {
                pid: i32::MAX,
                token: "gone".to_string(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(admission_lock_is_stale(&lock_path).unwrap());
        let snapshot = claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["a"])).unwrap();
        assert_eq!(snapshot.used, 1);
        // The reclaiming holder released its own lock on the way out.
        assert!(!lock_path.exists());
    }

    /// Upstream releases the admission lock in a `finally` (`run-fanout-budget.ts:201-204`), so an
    /// operation that THROWS still frees it. `operation` here reaches the caller-supplied `commit`
    /// closure of `claim_run_fanout_batch_with_commit_in`, so a panic there must release too:
    /// otherwise `admission.lock` is left behind owned by a LIVE pid, `admission_lock_is_stale`
    /// never reclaims it, and every later admission for that run fails with "Timed out acquiring
    /// run fan-out admission lock" until the process exits.
    #[test]
    fn a_panic_inside_the_commit_closure_releases_the_lock_and_rolls_back_its_claims() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-panic", 4).unwrap();
        let lock_path = descriptor.directory.join("admission.lock");

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            claim_run_fanout_batch_with_commit_in(
                root.path(),
                &descriptor,
                &paths(&["a"]),
                &no_wait,
                |_snapshot| panic!("the caller's commit exploded"),
            )
        }));
        std::panic::set_hook(previous);
        assert!(caught.is_err(), "the panic must not be swallowed");

        assert!(
            !lock_path.exists(),
            "the admission lock survived a panicking commit"
        );
        // And the panicking batch's claim was ROLLED BACK, so the next batch is the run's first
        // used slot rather than its second. Upstream unlinks every slot in `created` from the
        // `catch` that a throwing `commit` lands in (`run-fanout-budget.ts:257-262` @v0.64.0);
        // a claim is permanent once it stands (this module's reset boundary), so leaving it would
        // burn a slot of the run's cap for good on a caller bug.
        let snapshot = claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["b"])).unwrap();
        assert_eq!(
            snapshot.used, 1,
            "the panicking batch's claim must not survive its own unwind"
        );
    }

    #[test]
    fn the_doctor_block_reports_the_configured_limit_and_the_rung_it_came_from() {
        let root = tempfile::tempdir().unwrap();
        let none = |_: &str| None;
        assert_eq!(
            RunFanoutDoctor::resolve_with(&none, root.path(), None),
            RunFanoutDoctor::Configured {
                limit: 64,
                source: MaxSpawnsPerRunSource::Default
            }
        );
        assert_eq!(
            RunFanoutDoctor::resolve_with(&none, root.path(), None).lines(),
            vec![
                "- configured limit: 64 (default)".to_string(),
                "- usage: available after a run starts".to_string(),
                // The operator is told the cap is not applied yet — see the batch-4 review note
                // on `RunFanoutDoctor::lines`.
                "- enforcement: NOT WIRED — no run creates or claims against this budget yet, so \
                 the limit above is reported but not applied"
                    .to_string(),
                "- reset boundary: cumulative claims are never released; a new top-level run \
                 creates a new budget"
                    .to_string(),
            ]
        );
        assert_eq!(
            RunFanoutDoctor::resolve_with(&none, root.path(), Some(9)).lines()[0],
            "- configured limit: 9 (config)"
        );
        let env = |key: &str| (key == MAX_SPAWNS_PER_RUN_ENV).then(|| "5".to_string());
        assert_eq!(
            RunFanoutDoctor::resolve_with(&env, root.path(), Some(9)).lines()[0],
            "- configured limit: 5 (environment)"
        );
    }

    #[test]
    fn the_doctor_block_reports_an_inherited_ledgers_live_usage() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "root-run-7", 4).unwrap();
        claim_run_fanout_batch_in(root.path(), &descriptor, &paths(&["a"])).unwrap();
        let encoded = encode_run_fanout_budget_descriptor_in(root.path(), &descriptor).unwrap();
        let env = |key: &str| (key == RUN_FANOUT_BUDGET_ENV).then(|| encoded.clone());
        let block = RunFanoutDoctor::resolve_with(&env, root.path(), Some(9));
        assert_eq!(
            block.lines(),
            vec![
                "- usage: 1/4 used, 3 remaining".to_string(),
                "- root run: root-run-7".to_string(),
                "- enforcement: NOT WIRED — no run creates or claims against this budget yet, so \
                 the limit above is reported but not applied"
                    .to_string(),
                "- reset boundary: cumulative claims are never released; a new top-level run \
                 creates a new budget"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn a_broken_inheritance_is_reported_as_invalid_never_as_a_fresh_configured_limit() {
        let root = tempfile::tempdir().unwrap();
        let env = |key: &str| (key == RUN_FANOUT_BUDGET_ENV).then(|| "!!!".to_string());
        let block = RunFanoutDoctor::resolve_with(&env, root.path(), Some(9));
        let lines = block.lines();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with(
                "- inherited budget: invalid — Invalid inherited run fan-out budget: "
            ),
            "got {}",
            lines[0]
        );
    }

    #[test]
    fn the_doctor_block_consults_the_pi_env_alias_only_when_the_cyrup_spelling_is_unset() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "root-run-8", 2).unwrap();
        let encoded = encode_run_fanout_budget_descriptor_in(root.path(), &descriptor).unwrap();
        let alias_only =
            |key: &str| (key == RUN_FANOUT_BUDGET_ENV_PI_ALIAS).then(|| encoded.clone());
        assert!(matches!(
            RunFanoutDoctor::resolve_with(&alias_only, root.path(), None),
            RunFanoutDoctor::Inherited { .. }
        ));
    }

    #[test]
    fn an_ownerless_lock_younger_than_the_staleness_window_is_not_stale() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = create_run_fanout_budget_in(root.path(), "run-1", 2).unwrap();
        let lock_path = descriptor.directory.join("admission.lock");
        create_private_dir(&lock_path).unwrap();
        assert!(!admission_lock_is_stale(&lock_path).unwrap());
    }
}
