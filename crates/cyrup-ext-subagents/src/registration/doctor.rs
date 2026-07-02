//! `/subagents-doctor` concurrent check runner (func-SA §5.6 R-SA-131; arch-SA §6.8,
//! `registration/doctor.rs`).
//!
//! # What this module implements
//!
//! [`DoctorRunner::run`] executes, **concurrently** (`tokio::join!` over independent, read-only
//! probes — never sequentially), the six checks R-SA-131 mandates at minimum:
//!
//! (a) [`check_binary_resolution`] — resolvability of the `cyrup` binary used for subagent
//!     re-exec, per R-SA-045's three-tier resolution (`crate::spawn::resolve_spawn_command`).
//!     Per arch-SA §6.8's "Doctor check sequencing" note, this is the ONE check that spawns a
//!     subprocess: a short-timeout `<binary> --version` probe, matching R-SA-045's resolution
//!     intent without requiring a real model-probe spawn.
//! (b) [`check_temp_dir_writable`] — writability of the temp-scope root directory used for
//!     async run/artifact storage (the `async_root`/`AsyncRoot` this crate's `background::`
//!     subsystem writes `status.json`/`events.jsonl`/etc. under).
//! (c) [`check_config_json`] — presence and parse-validity of `config.json`
//!     ([`crate::registration::SubagentExtensionConfig`]'s on-disk form).
//! (d) [`check_agent_discovery`] — presence/count of discovered agent persona files across all
//!     scopes, via [`crate::discovery::discover_agents_all`] (Phase 2, already written) — reusing
//!     that entry point rather than re-implementing any discovery/walk logic here.
//! (e) [`check_chain_discovery`] — presence/count of discovered chain files, via the SAME
//!     [`crate::discovery::discover_agents_all`] call's `chains` field (one discovery pass covers
//!     both (d) and (e); this module does not run discovery twice).
//! (f) [`check_provider_catalog_freshness`] — provider/model catalog freshness per configured
//!     agent override. Per func-SA §5.6 R-SA-131's own parenthetical and arch-SA §12 item 11, this
//!     intentionally reports only catalog **freshness** (an mtime/staleness stat against the
//!     already-discovered agents' configured models), never catalog **content** — live model-probe
//!     spawning and the catalog-diffing/generation algorithm itself
//!     (`/subagents-refresh-provider-models`, `/subagents-generate-profiles`,
//!     `/subagents-check-profile`) are explicitly deferred to a follow-up addendum (func-SA §9 item
//!     31; arch-SA §12 item 11) — **not implemented in this file, and not implemented anywhere in
//!     this crate as of this phase.**
//!
//! Each check is independently reportable as Ok/Warn/Fail with a human-readable remedy string
//! (R-SA-131's own text) and **catches and records its own failure rather than aborting the whole
//! report** — this task's own framing. No check function in this module ever returns a bare
//! `Result` up to [`DoctorRunner::run`]'s caller; every fallible step inside a check is caught at
//! that check's own boundary and folded into a [`CheckStatus::Fail`]/[`CheckStatus::Warn`]
//! [`DoctorCheck`] instead, so one misconfigured check (e.g. a missing `config.json`) can never
//! prevent the other five from reporting.
//!
//! # Deferred to later phases (do not implement here)
//!
//! - Live model-probe checks (catalog *content*, not *freshness*) — func-SA §9 item 31, arch-SA
//!   §12 item 11. Owned by a future `registration/profiles.rs`-adjacent addendum, not this file.
//! - `/subagents-doctor`'s own slash-command descriptor/registration (the command name, help text,
//!   completions) — owned by `registration/slash_commands.rs`, a sibling phase of this crate's
//!   build-out that does not exist yet as of this task. This file exposes [`DoctorRunner`] as a
//!   plain, callable type; wiring it to the `/subagents-doctor` command name is that later phase's
//!   job, exactly as `registration/mod.rs`'s own doc comment already documents for
//!   `slash_commands.rs`/`profiles.rs`/`cost.rs`.
//! - Rendering a [`DoctorReport`] to a human-facing string (the actual `ctx.ui.notify`/terminal
//!   output shape) — left to whichever later phase wires this into `HostCtx` output, since this
//!   module has no dependency on `cyrup-tui`/`cyrup-ext`'s UI surface at all (arch-SA §2.1's
//!   crate-boundary rule). [`DoctorReport`] is a plain, `Display`-free data type here; a caller
//!   that wants a rendered string builds it from the public fields.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::discovery::{self, AgentDiscoveryConfig};
use crate::registration::SubagentExtensionConfig;
use crate::spawn::{self, SpawnCommand};

/// Bounded timeout for the one subprocess this module ever spawns (the binary-resolution
/// version-probe, check (a)) — matches this file's own "genuinely synchronous, short-lived
/// command" framing (arch-SA §6.8) rather than the much longer deadlines a real subagent run may
/// use elsewhere in this crate.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Staleness threshold for the provider-catalog-freshness check (f): a catalog cache file whose
/// mtime is older than this is reported `Warn`, not `Fail` — a stale catalog is a "you may want to
/// refresh this" signal, never a hard failure that would make `/subagents-doctor` non-conformant
/// just because an operator has not run `/subagents-refresh-provider-models` recently. Target: 7
/// days, generous enough that routine day-to-day use never trips it while still catching a catalog
/// that has clearly gone unmaintained.
const CATALOG_STALE_THRESHOLD: Duration = Duration::from_secs(7 * 24 * 60 * 60);

// =================================================================================================
// CheckStatus / DoctorCheck / DoctorReport (func-SA §4's illustrative `DoctorReport` shape)
// =================================================================================================

/// One check's outcome (func-SA §4: `DoctorCheck{name, status: Ok|Warn|Fail, detail, remedy}`).
///
/// `Ok` means the check found nothing actionable; `Warn` means a non-fatal, "you may want to look
/// at this" condition (e.g. zero agents discovered, a stale provider catalog); `Fail` means a
/// condition that will actually break subagent execution (e.g. the temp-scope root is not
/// writable, `config.json` fails to parse).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    /// The check found nothing actionable.
    Ok,
    /// A non-fatal, advisory condition.
    Warn,
    /// A condition that will actually break subagent execution.
    Fail,
}

impl CheckStatus {
    /// `true` for [`CheckStatus::Warn`] or [`CheckStatus::Fail`] — the two statuses A-SA-16
    /// expects a deliberately misconfigured environment to produce for exactly the checks that are
    /// actually misconfigured.
    #[must_use]
    pub fn is_actionable(self) -> bool {
        !matches!(self, CheckStatus::Ok)
    }
}

impl std::fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        })
    }
}

/// One diagnostic check's full result (func-SA §4 `DoctorCheck`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DoctorCheck {
    /// A short, stable, machine-referenceable name for this check (e.g. `"binary-resolution"`) —
    /// stable across releases so tooling/tests can assert on a specific check by name rather than
    /// by its position in the report's `Vec`.
    pub name: String,
    /// This check's outcome.
    pub status: CheckStatus,
    /// A human-readable sentence describing what was found (present regardless of status — even
    /// an `Ok` check reports what it verified, e.g. `"12 agents discovered"`).
    pub detail: String,
    /// A human-readable remedy string, present whenever `status != Ok` (R-SA-131: "Each check MUST
    /// be independently reportable as Ok/Warn/Fail with a human-readable remedy string"). `None`
    /// for an `Ok` check, which has nothing to remedy.
    pub remedy: Option<String>,
}

impl DoctorCheck {
    /// Constructs an `Ok` check with no remedy.
    #[must_use]
    pub fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Ok,
            detail: detail.into(),
            remedy: None,
        }
    }

    /// Constructs a `Warn` check with a remedy.
    #[must_use]
    pub fn warn(
        name: impl Into<String>,
        detail: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Warn,
            detail: detail.into(),
            remedy: Some(remedy.into()),
        }
    }

    /// Constructs a `Fail` check with a remedy.
    #[must_use]
    pub fn fail(
        name: impl Into<String>,
        detail: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Fail,
            detail: detail.into(),
            remedy: Some(remedy.into()),
        }
    }
}

/// The full `/subagents-doctor` report: every check's result, in the fixed order
/// [`DoctorRunner::run`] assembles them (a..f per R-SA-131's own lettering) — never reordered by
/// completion order, even though the checks themselves run concurrently (mirrors this crate's
/// established "concurrent execution, deterministic result ordering" convention, e.g.
/// `spawn::parallel`'s pre-sized `Vec<Option<StepResult>>` indexed by original position).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DoctorReport {
    /// Every check's result, in fixed a..f order.
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// `true` if every check reported `Ok`.
    #[must_use]
    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(|c| c.status == CheckStatus::Ok)
    }

    /// Every check whose status is `Warn` or `Fail` (A-SA-16's "exactly the expected Warn/Fail
    /// subset" assertion target).
    #[must_use]
    pub fn actionable(&self) -> Vec<&DoctorCheck> {
        self.checks
            .iter()
            .filter(|c| c.status.is_actionable())
            .collect()
    }

    /// Look up one check by its stable [`DoctorCheck::name`].
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&DoctorCheck> {
        self.checks.iter().find(|c| c.name == name)
    }
}

// =================================================================================================
// Stable check names (used by DoctorCheck::name / DoctorReport::find, and by this module's tests)
// =================================================================================================

/// Check (a): binary resolvability.
pub const CHECK_BINARY_RESOLUTION: &str = "binary-resolution";
/// Check (b): temp-scope root writability.
pub const CHECK_TEMP_DIR_WRITABLE: &str = "temp-dir-writable";
/// Check (c): `config.json` presence/parse-validity.
pub const CHECK_CONFIG_JSON: &str = "config-json";
/// Check (d): discovered agent persona file count.
pub const CHECK_AGENT_DISCOVERY: &str = "agent-discovery";
/// Check (e): discovered chain file count.
pub const CHECK_CHAIN_DISCOVERY: &str = "chain-discovery";
/// Check (f): provider/model catalog freshness.
pub const CHECK_PROVIDER_CATALOG_FRESHNESS: &str = "provider-catalog-freshness";

// =================================================================================================
// DoctorRunner
// =================================================================================================

/// Everything one [`DoctorRunner::run`] call needs, assembled by the caller (normally
/// `extension.rs`'s `/subagents-doctor` command handler, a later phase) from cyrup's own resolved
/// directory/settings state — mirrors [`AgentDiscoveryConfig`]'s own "caller assembles already-
/// resolved paths, this type performs no directory-resolution of its own" convention.
#[derive(Clone, Debug)]
pub struct DoctorRunner {
    /// The temp-scope root directory used for async run/artifact storage (check b) — the same
    /// `async_root` this crate's `background::` subsystem writes `status.json`/`events.jsonl`/etc.
    /// under (plain `&Path`, not a dedicated wrapper type, matching `background::control`'s own
    /// established convention of threading `async_root: &Path` through every call site rather than
    /// introducing an `AsyncRoot` newtype).
    pub async_root: PathBuf,
    /// Path to this extension's `config.json` (check c) — the on-disk form of
    /// [`SubagentExtensionConfig`].
    pub config_json_path: PathBuf,
    /// Agent/chain discovery configuration (checks d/e), already resolved by the caller exactly as
    /// [`discovery::discover_agents_all`]'s own doc comment expects.
    pub discovery_config: AgentDiscoveryConfig,
    /// Path to the provider/model catalog cache file (check f), if one is configured. `None` means
    /// no catalog has ever been generated (`/subagents-generate-profiles` has never been run for
    /// this installation) — a normal, `Warn`-not-`Fail` condition, not a missing-config error.
    pub provider_catalog_path: Option<PathBuf>,
}

impl DoctorRunner {
    /// Run all six R-SA-131 checks **concurrently** (`tokio::join!` over independent, read-only
    /// probes — arch-SA §6.8's "Doctor check sequencing" note: "all checks run concurrently... then
    /// render one `DoctorReport` synchronously"), returning one [`DoctorReport`] in fixed a..f
    /// order. Never fails: every check catches its own errors internally (see this module's own
    /// top-level doc), so this method has no `Result` return type at all — a caller can always
    /// render *something*, even in a maximally broken environment.
    pub async fn run(&self) -> DoctorReport {
        let (binary, temp_dir, config, discovery_result, catalog) = tokio::join!(
            check_binary_resolution(),
            check_temp_dir_writable(&self.async_root),
            check_config_json(&self.config_json_path),
            run_discovery_checks(&self.discovery_config),
            check_provider_catalog_freshness(
                self.provider_catalog_path.as_deref(),
                &self.discovery_config,
            ),
        );

        let (agents, chains) = discovery_result;

        DoctorReport {
            checks: vec![binary, temp_dir, config, agents, chains, catalog],
        }
    }
}

// =================================================================================================
// (a) Binary resolution (R-SA-045)
// =================================================================================================

/// Check (a): resolvability of the `cyrup` binary used for subagent re-exec, per R-SA-045's
/// three-tier resolution ([`spawn::resolve_spawn_command`]). Per arch-SA §6.8, this is the ONE
/// check in this module that spawns a subprocess: a short-timeout `<binary> --version` probe,
/// bounded by [`VERSION_PROBE_TIMEOUT`] — proving the resolved binary is not merely a path that
/// exists on disk, but one that actually executes and responds, without needing a real model-probe
/// spawn (that is explicitly out of scope here, see this module's top doc).
///
/// - `Fail` if the resolved path does not exist at all (tier 2/3 resolved to nothing usable) —
///   every subagent spawn will fail immediately.
/// - `Warn` if the resolved path exists but the version-probe subprocess did not exit
///   successfully within [`VERSION_PROBE_TIMEOUT]` (spawn failure, non-zero exit, or timeout) —
///   the binary MIGHT still work for a real subagent invocation (a `--version` flag is not
///   guaranteed universally supported by every possible `CYRUP_SUBAGENT_BINARY` override a caller
///   could configure), so this is advisory, not fatal.
/// - `Ok` if the probe subprocess spawns and exits successfully within the timeout.
async fn check_binary_resolution() -> DoctorCheck {
    let resolved = spawn::resolve_spawn_command();
    check_binary_resolution_for(&resolved).await
}

/// The pure(r) core of [`check_binary_resolution`], parameterized over the already-resolved
/// [`SpawnCommand`] so this module's own tests can exercise every branch (missing binary, binary
/// that exists but fails `--version`, binary that succeeds) without depending on
/// `spawn::resolve_spawn_command`'s ambient real-environment resolution.
async fn check_binary_resolution_for(resolved: &SpawnCommand) -> DoctorCheck {
    // A relative/bare binary name (tier-3 PATH fallback, e.g. literal "cyrup") is not something
    // this check can `Path::exists()` against directly — it is resolved by the OS/tokio at spawn
    // time via PATH lookup, which is exactly what the version-probe subprocess spawn below already
    // exercises. Only an ABSOLUTE resolved path (the common tier-1/tier-2 case) is checked for
    // on-disk existence up front, so a bare `PATH`-relative name does not spuriously `Fail` this
    // check before ever attempting the probe.
    if resolved.binary.is_absolute() && !resolved.binary.exists() {
        return DoctorCheck::fail(
            CHECK_BINARY_RESOLUTION,
            format!(
                "resolved subagent binary does not exist: {}",
                resolved.binary.display()
            ),
            format!(
                "verify CYRUP_SUBAGENT_BINARY (if set) points at a real executable, or that \
                 std::env::current_exe() resolves correctly for this installation; resolved path \
                 was {}",
                resolved.binary.display()
            ),
        );
    }

    let mut argv = resolved.base_args.clone();
    argv.push("--version".to_string());

    let probe = tokio::process::Command::new(&resolved.binary)
        .args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match tokio::time::timeout(VERSION_PROBE_TIMEOUT, probe).await {
        Ok(Ok(status)) if status.success() => DoctorCheck::ok(
            CHECK_BINARY_RESOLUTION,
            format!(
                "subagent re-exec binary resolved and responded: {}",
                resolved.binary.display()
            ),
        ),
        Ok(Ok(status)) => DoctorCheck::warn(
            CHECK_BINARY_RESOLUTION,
            format!(
                "resolved binary {} exited non-zero ({status}) on --version probe",
                resolved.binary.display()
            ),
            "confirm the resolved binary is a working cyrup executable; a non-zero --version \
             exit does not necessarily prevent subagent spawns, but is worth investigating"
                .to_string(),
        ),
        Ok(Err(err)) => DoctorCheck::warn(
            CHECK_BINARY_RESOLUTION,
            format!(
                "failed to spawn resolved binary {} for --version probe: {err}",
                resolved.binary.display()
            ),
            "verify CYRUP_SUBAGENT_BINARY (if set) or the current_exe()-resolved path is \
             executable"
                .to_string(),
        ),
        Err(_elapsed) => DoctorCheck::warn(
            CHECK_BINARY_RESOLUTION,
            format!(
                "--version probe against {} did not complete within {:?}",
                resolved.binary.display(),
                VERSION_PROBE_TIMEOUT
            ),
            "the resolved binary may be hung, extremely slow to start, or waiting on stdin; \
             investigate before relying on background subagent runs"
                .to_string(),
        ),
    }
}

// =================================================================================================
// (b) Temp-scope root writability
// =================================================================================================

/// Check (b): writability of the temp-scope root directory used for async run/artifact storage.
///
/// Performed as a real write-probe (create a uniquely named file inside `async_root`, write a
/// handful of bytes, then remove it) rather than a permission-bit inspection — this crate is
/// `#![forbid(unsafe_code)]` and therefore does not call the `access(2)`-class syscalls
/// `cyrup_tools::ops::local::LocalFs::access` uses on Unix; a real write-then-delete probe is
/// portable across platforms, immune to permission-bit/ACL/read-only-filesystem edge cases a
/// metadata-only check could miss, and matches this crate's "spawns real subprocesses, exercises
/// real filesystem behavior" testing convention (this check is not itself a test, but the same
/// "trust the actual OS outcome" philosophy applies).
///
/// - `Fail` if `async_root` does not exist AND cannot be created (parent missing/unwritable), or
///   exists but a real file write inside it fails (permission denied, read-only filesystem, out of
///   space, etc.) — background async runs cannot function without this.
/// - `Ok` if the write-probe round-trips successfully (the probe file is written and removed
///   without error).
async fn check_temp_dir_writable(async_root: &Path) -> DoctorCheck {
    if let Err(err) = tokio::fs::create_dir_all(async_root).await {
        return DoctorCheck::fail(
            CHECK_TEMP_DIR_WRITABLE,
            format!(
                "temp-scope root {} does not exist and could not be created: {err}",
                async_root.display()
            ),
            format!(
                "create {} manually, or point the async-run temp-scope root at a writable \
                 directory",
                async_root.display()
            ),
        );
    }

    let probe_name = format!(".cyrup-subagents-doctor-probe-{}", uuid::Uuid::new_v4().as_simple());
    let probe_path = async_root.join(probe_name);

    let write_result = tokio::fs::write(&probe_path, b"doctor-write-probe").await;
    match write_result {
        Ok(()) => {
            // Best-effort cleanup: a failure to remove the probe file does not itself indicate a
            // writability problem (the write above already succeeded, which is the property this
            // check exists to verify) — mirrors `spawn::cleanup_temp_files`'s identical
            // "best-effort, log-don't-fail" convention for its own temp-file removal.
            if let Err(err) = tokio::fs::remove_file(&probe_path).await {
                tracing::warn!(
                    path = %probe_path.display(),
                    error = %err,
                    "doctor temp-dir write-probe file could not be removed after a successful \
                     write; leaving it in place"
                );
            }
            DoctorCheck::ok(
                CHECK_TEMP_DIR_WRITABLE,
                format!("temp-scope root is writable: {}", async_root.display()),
            )
        }
        Err(err) => DoctorCheck::fail(
            CHECK_TEMP_DIR_WRITABLE,
            format!(
                "temp-scope root {} is not writable: {err}",
                async_root.display()
            ),
            format!(
                "check permissions/ownership on {}, or reconfigure the async-run temp-scope root \
                 to a writable directory — background subagent runs cannot persist status/results \
                 without this",
                async_root.display()
            ),
        ),
    }
}

// =================================================================================================
// (c) config.json presence/parse-validity
// =================================================================================================

/// Check (c): presence and parse-validity of `config.json`
/// ([`SubagentExtensionConfig`]'s on-disk form).
///
/// - `Warn` if the file is simply absent — a missing `config.json` is not fatal (tier 5's
///   hardcoded [`SubagentExtensionConfig::default`] applies per R-SA-133), but is worth surfacing
///   so an operator who *intended* to customize it notices the file never got created.
/// - `Fail` if the file exists but is not valid JSON, or does not parse as
///   [`SubagentExtensionConfig`] — the extension will not be able to honor whatever customization
///   the operator intended, and (depending on the caller's own load-time error handling) may
///   surface a confusing downstream error rather than this precise one.
/// - `Ok` if the file exists and parses successfully.
async fn check_config_json(config_json_path: &Path) -> DoctorCheck {
    let contents = match tokio::fs::read_to_string(config_json_path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return DoctorCheck::warn(
                CHECK_CONFIG_JSON,
                format!("config.json not found at {}", config_json_path.display()),
                format!(
                    "this is not fatal — hardcoded defaults apply — but if you intended to \
                     customize subagent settings, create {}",
                    config_json_path.display()
                ),
            );
        }
        Err(err) => {
            return DoctorCheck::fail(
                CHECK_CONFIG_JSON,
                format!(
                    "config.json at {} could not be read: {err}",
                    config_json_path.display()
                ),
                format!(
                    "check permissions on {}",
                    config_json_path.display()
                ),
            );
        }
    };

    match serde_json::from_str::<SubagentExtensionConfig>(&contents) {
        Ok(_) => DoctorCheck::ok(
            CHECK_CONFIG_JSON,
            format!(
                "config.json at {} is present and parses successfully",
                config_json_path.display()
            ),
        ),
        Err(err) => DoctorCheck::fail(
            CHECK_CONFIG_JSON,
            format!(
                "config.json at {} failed to parse: {err}",
                config_json_path.display()
            ),
            format!(
                "fix the JSON syntax/shape at {}, or remove the file to fall back to hardcoded \
                 defaults",
                config_json_path.display()
            ),
        ),
    }
}

// =================================================================================================
// (d)/(e) Agent + chain discovery counts — ONE discovery pass covers both
// =================================================================================================

/// Runs [`discovery::discover_agents_all`] exactly once and derives both check (d) (agent count)
/// and check (e) (chain count) from its single result, so this module never pays the cost of (or
/// risks the two counts disagreeing from) running discovery twice.
///
/// Both checks are caught independently at this function's own boundary (a discovery-level error —
/// R-SA-009's "malformed subagents settings MUST abort discovery" case — surfaces as `Fail` on
/// BOTH (d) and (e), since a single discovery pass covers both and a failure here means neither
/// count was obtainable), matching this module's "each check catches its own failure" contract
/// even though the two checks share one underlying computation.
async fn run_discovery_checks(cfg: &AgentDiscoveryConfig) -> (DoctorCheck, DoctorCheck) {
    // Discovery (`discovery::run_discovery`'s callees) is synchronous, real filesystem I/O
    // (R-SA-019: re-scanned per call, never cached) — run it on a blocking-safe spawn so it never
    // blocks this async check's siblings running concurrently in the same `tokio::join!`. The
    // discovery config itself needs to be owned by the spawned task; `AgentDiscoveryConfig`
    // derives `Clone` (arch-SA §3.3), so a cheap clone here is well within this check's own
    // "read-only, independent probe" framing.
    let cfg_owned = cfg.clone();
    let result = tokio::task::spawn_blocking(move || discovery::discover_agents_all(&cfg_owned))
        .await;

    match result {
        Ok(Ok(discovered)) => {
            let agent_count = discovered.agents.len();
            let chain_count = discovered.chains.len();

            let agents_check = if agent_count == 0 {
                DoctorCheck::warn(
                    CHECK_AGENT_DISCOVERY,
                    "0 agent persona files discovered across all scopes",
                    "verify builtin_agents_dir/user/project agent directories are configured and \
                     contain valid .md persona files with name/description frontmatter"
                        .to_string(),
                )
            } else {
                DoctorCheck::ok(
                    CHECK_AGENT_DISCOVERY,
                    format!("{agent_count} agent persona file(s) discovered across all scopes"),
                )
            };

            // A zero chain count is normal and expected (chains are an optional, opt-in feature —
            // unlike agents, a working subagent installation with zero chain files is entirely
            // unremarkable), so this is `Ok`, never `Warn`, regardless of count.
            let chains_check = DoctorCheck::ok(
                CHECK_CHAIN_DISCOVERY,
                format!("{chain_count} chain file(s) discovered across all scopes"),
            );

            (agents_check, chains_check)
        }
        Ok(Err(err)) => {
            let detail = format!("agent/chain discovery failed: {err}");
            let remedy = "check subagents settings (malformed subagents.* settings abort \
                           discovery entirely per R-SA-009) and agent-directory permissions"
                .to_string();
            (
                DoctorCheck::fail(CHECK_AGENT_DISCOVERY, detail.clone(), remedy.clone()),
                DoctorCheck::fail(CHECK_CHAIN_DISCOVERY, detail, remedy),
            )
        }
        Err(join_err) => {
            let detail = format!("agent/chain discovery task panicked or was cancelled: {join_err}");
            let remedy = "this indicates a bug in discovery itself rather than a configuration \
                           problem; please report it"
                .to_string();
            (
                DoctorCheck::fail(CHECK_AGENT_DISCOVERY, detail.clone(), remedy.clone()),
                DoctorCheck::fail(CHECK_CHAIN_DISCOVERY, detail, remedy),
            )
        }
    }
}

// =================================================================================================
// (f) Provider/model catalog freshness
// =================================================================================================

/// Check (f): provider/model catalog freshness per configured agent override.
///
/// Per R-SA-131's own parenthetical ("Live model-probe checks are explicitly deferred — see §9")
/// and arch-SA §6.8/§12 item 11, this reports only catalog **freshness** (an mtime/staleness stat
/// on the catalog cache file), never catalog **content** — validating that specific configured
/// models actually exist in the catalog, or spawning a live probe against a provider, is the
/// deferred `/subagents-refresh-provider-models`/`/subagents-generate-profiles`/
/// `/subagents-check-profile` algorithm (func-SA §9 item 31), NOT implemented here or anywhere
/// else in this crate as of this phase.
///
/// "Per configured agent override": this check only fires at all when at least one discovered
/// agent actually configures a `model`/`fallback_models` override (an installation with zero
/// model-overriding agents has nothing for a provider catalog to be stale/fresh FOR) — mirroring
/// R-SA-131's own wording ("freshness per configured agent override").
///
/// - `Ok` (with a "no model overrides configured" detail, not a Warn) if no discovered agent
///   configures any `model`/`fallback_models` override — there is nothing to check freshness for.
/// - `Warn` if agents DO configure model overrides but no catalog cache file exists yet, or exists
///   but is older than [`CATALOG_STALE_THRESHOLD`] — advisory, since a missing/stale catalog does
///   not itself break execution (model names are still passed through to the child verbatim; the
///   catalog is a UX/validation aid, not a hard runtime dependency).
/// - `Fail` is never returned by this check — a stale or absent catalog is, by this check's own
///   scope (freshness only, never content/existence-validation), always advisory at worst.
async fn check_provider_catalog_freshness(
    provider_catalog_path: Option<&Path>,
    discovery_cfg: &AgentDiscoveryConfig,
) -> DoctorCheck {
    let cfg_owned = discovery_cfg.clone();
    let has_model_overrides = tokio::task::spawn_blocking(move || {
        discovery::discover_agents_all(&cfg_owned)
            .map(|result| {
                result
                    .agents
                    .iter()
                    .any(|agent| agent.model.is_some() || !agent.fallback_models.is_empty())
            })
            .unwrap_or(false) // a discovery failure here is already fully reported by (d)/(e); this
                               // check degrades to "nothing to check" rather than duplicating that
                               // failure a third time.
    })
    .await
    .unwrap_or(false);

    if !has_model_overrides {
        return DoctorCheck::ok(
            CHECK_PROVIDER_CATALOG_FRESHNESS,
            "no discovered agent configures a model/fallback-models override; nothing to check \
             catalog freshness against"
                .to_string(),
        );
    }

    let Some(catalog_path) = provider_catalog_path else {
        return DoctorCheck::warn(
            CHECK_PROVIDER_CATALOG_FRESHNESS,
            "one or more agents configure a model override, but no provider/model catalog has \
             ever been generated"
                .to_string(),
            "run /subagents-generate-profiles to build a provider/model catalog cache".to_string(),
        );
    };

    let metadata = match tokio::fs::metadata(catalog_path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return DoctorCheck::warn(
                CHECK_PROVIDER_CATALOG_FRESHNESS,
                format!(
                    "provider catalog cache configured at {} but the file does not exist",
                    catalog_path.display()
                ),
                "run /subagents-generate-profiles to (re)build the provider/model catalog cache"
                    .to_string(),
            );
        }
        Err(err) => {
            return DoctorCheck::warn(
                CHECK_PROVIDER_CATALOG_FRESHNESS,
                format!(
                    "provider catalog cache at {} could not be inspected: {err}",
                    catalog_path.display()
                ),
                format!("check permissions on {}", catalog_path.display()),
            );
        }
    };

    let modified = match metadata.modified() {
        Ok(modified) => modified,
        Err(err) => {
            return DoctorCheck::warn(
                CHECK_PROVIDER_CATALOG_FRESHNESS,
                format!(
                    "provider catalog cache at {} has no readable modification time on this \
                     platform: {err}",
                    catalog_path.display()
                ),
                "freshness cannot be determined on this platform/filesystem; treat the catalog \
                 as potentially stale"
                    .to_string(),
            );
        }
    };

    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO); // a modified-in-the-future timestamp (clock skew) is treated
                                     // as zero age rather than underflowing/erroring.

    if age > CATALOG_STALE_THRESHOLD {
        DoctorCheck::warn(
            CHECK_PROVIDER_CATALOG_FRESHNESS,
            format!(
                "provider catalog cache at {} is {} old (stale threshold: {})",
                catalog_path.display(),
                humanize_duration(age),
                humanize_duration(CATALOG_STALE_THRESHOLD),
            ),
            "run /subagents-refresh-provider-models to refresh the provider/model catalog cache"
                .to_string(),
        )
    } else {
        DoctorCheck::ok(
            CHECK_PROVIDER_CATALOG_FRESHNESS,
            format!(
                "provider catalog cache at {} is fresh ({} old)",
                catalog_path.display(),
                humanize_duration(age),
            ),
        )
    }
}

/// Coarse, human-readable duration rendering for doctor detail/remedy strings (e.g. "3d 4h",
/// "45m") — deliberately not a general-purpose formatter, just enough precision for an operator
/// skimming a doctor report.
fn humanize_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let days = total_secs / 86_400;
    let hours = (total_secs % 86_400) / 3_600;
    let minutes = (total_secs % 3_600) / 60;

    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{total_secs}s")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    // -----------------------------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------------------------

    fn write_agent(dir: &Path, file_name: &str, name: &str) {
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(
            dir.join(file_name),
            format!("---\nname: {name}\ndescription: d\n---\n\nBody.\n"),
        )
        .expect("write agent file");
    }

    fn write_agent_with_model(dir: &Path, file_name: &str, name: &str, model: &str) {
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(
            dir.join(file_name),
            format!("---\nname: {name}\ndescription: d\nmodel: {model}\n---\n\nBody.\n"),
        )
        .expect("write agent file");
    }

    fn empty_discovery_config() -> AgentDiscoveryConfig {
        AgentDiscoveryConfig::default()
    }

    // -----------------------------------------------------------------------------------------
    // CheckStatus / DoctorCheck / DoctorReport plumbing
    // -----------------------------------------------------------------------------------------

    #[test]
    fn check_status_is_actionable_excludes_only_ok() {
        assert!(!CheckStatus::Ok.is_actionable());
        assert!(CheckStatus::Warn.is_actionable());
        assert!(CheckStatus::Fail.is_actionable());
    }

    #[test]
    fn doctor_check_ok_has_no_remedy() {
        let check = DoctorCheck::ok("x", "detail");
        assert_eq!(check.status, CheckStatus::Ok);
        assert!(check.remedy.is_none());
    }

    #[test]
    fn doctor_check_warn_and_fail_always_carry_a_remedy() {
        let warn = DoctorCheck::warn("x", "detail", "remedy");
        assert_eq!(warn.status, CheckStatus::Warn);
        assert_eq!(warn.remedy.as_deref(), Some("remedy"));

        let fail = DoctorCheck::fail("x", "detail", "remedy");
        assert_eq!(fail.status, CheckStatus::Fail);
        assert_eq!(fail.remedy.as_deref(), Some("remedy"));
    }

    #[test]
    fn doctor_report_all_ok_true_only_when_every_check_is_ok() {
        let report = DoctorReport {
            checks: vec![DoctorCheck::ok("a", "d"), DoctorCheck::ok("b", "d")],
        };
        assert!(report.all_ok());

        let report_with_warn = DoctorReport {
            checks: vec![
                DoctorCheck::ok("a", "d"),
                DoctorCheck::warn("b", "d", "r"),
            ],
        };
        assert!(!report_with_warn.all_ok());
    }

    #[test]
    fn doctor_report_actionable_returns_exactly_the_warn_and_fail_subset() {
        let report = DoctorReport {
            checks: vec![
                DoctorCheck::ok("ok-check", "d"),
                DoctorCheck::warn("warn-check", "d", "r"),
                DoctorCheck::fail("fail-check", "d", "r"),
            ],
        };
        let actionable_names: Vec<&str> =
            report.actionable().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(actionable_names, vec!["warn-check", "fail-check"]);
    }

    #[test]
    fn doctor_report_find_locates_by_stable_name() {
        let report = DoctorReport {
            checks: vec![DoctorCheck::ok(CHECK_BINARY_RESOLUTION, "d")],
        };
        assert!(report.find(CHECK_BINARY_RESOLUTION).is_some());
        assert!(report.find("nonexistent-check").is_none());
    }

    #[test]
    fn check_status_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&CheckStatus::Ok).unwrap(), "\"ok\"");
        assert_eq!(
            serde_json::to_string(&CheckStatus::Warn).unwrap(),
            "\"warn\""
        );
        assert_eq!(
            serde_json::to_string(&CheckStatus::Fail).unwrap(),
            "\"fail\""
        );
    }

    #[test]
    fn doctor_report_round_trips_through_json() {
        let report = DoctorReport {
            checks: vec![
                DoctorCheck::ok("a", "d"),
                DoctorCheck::warn("b", "d", "r"),
                DoctorCheck::fail("c", "d", "r"),
            ],
        };
        let json = serde_json::to_string(&report).expect("serializes");
        let back: DoctorReport = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, report);
    }

    // -----------------------------------------------------------------------------------------
    // (a) check_binary_resolution
    // -----------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn binary_resolution_fails_when_resolved_absolute_path_does_not_exist() {
        let resolved = SpawnCommand {
            binary: PathBuf::from("/definitely/does/not/exist/anywhere/cyrup-fake"),
            base_args: Vec::new(),
        };
        let check = check_binary_resolution_for(&resolved).await;
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.remedy.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn binary_resolution_ok_when_probe_succeeds() {
        // `true` (a real, universally present executable) exits 0 with no args — stands in for a
        // `--version`-supporting binary succeeding, since this test does not depend on a real
        // `cyrup` binary being built.
        let true_path = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("true"))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| PathBuf::from("/usr/bin/true"));

        // `true --version` still exits 0 (GNU coreutils' `true` ignores all arguments and always
        // exits 0), so this exercises the Ok path faithfully.
        let resolved = SpawnCommand {
            binary: true_path,
            base_args: Vec::new(),
        };
        let check = check_binary_resolution_for(&resolved).await;
        assert_eq!(check.status, CheckStatus::Ok);
        assert!(check.remedy.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn binary_resolution_warns_when_probe_exits_non_zero() {
        let false_path = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("false"))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| PathBuf::from("/usr/bin/false"));

        let resolved = SpawnCommand {
            binary: false_path,
            base_args: Vec::new(),
        };
        let check = check_binary_resolution_for(&resolved).await;
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.remedy.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn binary_resolution_warns_when_spawn_itself_fails() {
        // A relative, non-PATH-resolvable, nonexistent name: `is_absolute()` is false so the
        // up-front existence check is skipped, and the actual spawn attempt fails.
        let resolved = SpawnCommand {
            binary: PathBuf::from("cyrup-doctor-test-nonexistent-relative-binary"),
            base_args: Vec::new(),
        };
        let check = check_binary_resolution_for(&resolved).await;
        assert_eq!(check.status, CheckStatus::Warn);
    }

    // -----------------------------------------------------------------------------------------
    // (b) check_temp_dir_writable — including the deliberately-misconfigured (read-only) case
    // -----------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn temp_dir_writable_ok_for_a_normal_writable_directory() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let check = check_temp_dir_writable(dir.path()).await;
        assert_eq!(check.status, CheckStatus::Ok);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn temp_dir_writable_creates_a_missing_directory_and_reports_ok() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let nested = dir.path().join("nested").join("async-root");
        assert!(!nested.exists());
        let check = check_temp_dir_writable(&nested).await;
        assert_eq!(check.status, CheckStatus::Ok);
        assert!(nested.is_dir());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn temp_dir_writable_fails_for_a_real_read_only_directory() {
        // Real chmod against a real directory (this crate's own no-mocking convention) — R-SA-131
        // /A-SA-16's "unwritable temp root via chmod" fixture.
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("real tempdir");
        let ro_dir = dir.path().join("read-only-async-root");
        std::fs::create_dir_all(&ro_dir).expect("mkdir");
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o500))
            .expect("chmod read-only");

        let check = check_temp_dir_writable(&ro_dir).await;

        // Restore write permission so the tempdir guard can clean up on drop, regardless of the
        // assertion outcome below.
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o700))
            .expect("restore permissions for cleanup");

        // Root (common in some CI/container environments) bypasses the read-only bit entirely, so
        // this assertion is skipped rather than spuriously failing when running as uid 0.
        if !running_as_root() {
            assert_eq!(
                check.status,
                CheckStatus::Fail,
                "a real chmod'd-read-only directory must report Fail: {check:?}"
            );
            assert!(check.remedy.is_some());
        }
    }

    #[cfg(unix)]
    fn running_as_root() -> bool {
        // `libc::geteuid` would need `unsafe`, which this crate forbids; shelling out to `id -u`
        // is the same "spawn a real process, trust its real output" convention this crate already
        // uses elsewhere, applied to a one-off test-only environment probe.
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------------------------------
    // (c) check_config_json — including the deliberately-misconfigured (missing) case
    // -----------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn config_json_warns_when_file_is_missing() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let missing = dir.path().join("does-not-exist").join("config.json");
        let check = check_config_json(&missing).await;
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.remedy.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn config_json_ok_when_valid() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("config.json");
        let cfg = SubagentExtensionConfig::default();
        tokio::fs::write(&path, serde_json::to_vec(&cfg).unwrap())
            .await
            .expect("write config.json");

        let check = check_config_json(&path).await;
        assert_eq!(check.status, CheckStatus::Ok);
        assert!(check.remedy.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn config_json_fails_when_present_but_malformed() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("config.json");
        tokio::fs::write(&path, b"{ this is not valid json")
            .await
            .expect("write malformed config.json");

        let check = check_config_json(&path).await;
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.remedy.is_some());
    }

    // -----------------------------------------------------------------------------------------
    // (d)/(e) run_discovery_checks
    // -----------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovery_checks_warn_zero_agents_but_ok_zero_chains() {
        let cfg = empty_discovery_config();
        let (agents, chains) = run_discovery_checks(&cfg).await;
        assert_eq!(agents.status, CheckStatus::Warn, "{agents:?}");
        assert!(agents.detail.contains('0'));
        assert_eq!(
            chains.status,
            CheckStatus::Ok,
            "zero chains is normal, not actionable: {chains:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovery_checks_ok_when_agents_present() {
        let dir = tempfile::tempdir().expect("real tempdir");
        write_agent(dir.path(), "scout.md", "scout");
        write_agent(dir.path(), "worker.md", "worker");

        let cfg = AgentDiscoveryConfig {
            project_agent_dirs: vec![dir.path().to_path_buf()],
            ..AgentDiscoveryConfig::default()
        };

        let (agents, chains) = run_discovery_checks(&cfg).await;
        assert_eq!(agents.status, CheckStatus::Ok);
        assert!(agents.detail.contains('2'));
        assert_eq!(chains.status, CheckStatus::Ok);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovery_checks_report_chain_count() {
        let dir = tempfile::tempdir().expect("real tempdir");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.path().join("release.chain.json"),
            "{\"name\":\"release\",\"description\":\"d\",\"steps\":[]}",
        )
        .expect("write chain file");

        let cfg = AgentDiscoveryConfig {
            project_chain_dirs: vec![dir.path().to_path_buf()],
            ..AgentDiscoveryConfig::default()
        };

        let (_agents, chains) = run_discovery_checks(&cfg).await;
        assert_eq!(chains.status, CheckStatus::Ok);
        assert!(chains.detail.contains('1'));
    }

    #[test]
    fn malformed_settings_json_is_rejected_upstream_of_agent_discovery_config() {
        // `run_discovery_checks` maps a `discover_agents_all` `Err` to `Fail` on BOTH (d) and (e)
        // (see its own doc comment) — but `discovery::merge::discover_and_merge` (the sole `Err`
        // source `discover_agents_all` can propagate, per R-SA-009) never actually returns `Err`
        // for any state reachable through an already-typed `AgentDiscoveryConfig.settings:
        // SubagentSettings`, because `SubagentSettings` has no invalid state representable once
        // constructed (its `overrides: BTreeMap<String, AgentOverrideConfig>` and other fields are
        // already-validated typed values, not raw JSON). R-SA-009's actual malformed-settings
        // MUST-abort behavior lives one layer up, at the raw-JSON parse boundary
        // (`discovery::parse_subagent_settings`) — a caller assembling an `AgentDiscoveryConfig`
        // is contractually required to have already called that fallible parse first (see
        // `AgentDiscoveryConfig::settings`'s own doc: "a malformed value here is the caller's
        // problem to have already surfaced"). This test verifies that upstream boundary directly
        // (already covered exhaustively by `discovery/mod.rs`'s own
        // `parse_subagent_settings_malformed_shape_is_an_error` test; restated here only to
        // document — not silently assume — why `run_discovery_checks`'s Fail-mapping arm below has
        // no reachable unit-test fixture of its own under this crate's current type contracts).
        let raw = serde_json::json!({ "overrides": "not-an-object" });
        let result = discovery::parse_subagent_settings(Some(&raw));
        assert!(
            matches!(result, Err(crate::error::SubagentError::MalformedSettings(_))),
            "a caller MUST reject malformed subagents settings before ever constructing an \
             AgentDiscoveryConfig from them (R-SA-009) — this is the actual abort point, not \
             run_discovery_checks itself"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovery_checks_never_fail_on_the_ordinary_empty_config_path() {
        // Baseline sanity: the ordinary empty-config path used elsewhere in this file must not be
        // confused with a failure — zero agents is Warn, never Fail; zero chains is Ok.
        let cfg = empty_discovery_config();
        let (agents, chains) = run_discovery_checks(&cfg).await;
        assert_ne!(agents.status, CheckStatus::Fail);
        assert_ne!(chains.status, CheckStatus::Fail);
    }

    // -----------------------------------------------------------------------------------------
    // (f) check_provider_catalog_freshness
    // -----------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_freshness_ok_when_no_agent_configures_a_model_override() {
        let dir = tempfile::tempdir().expect("real tempdir");
        write_agent(dir.path(), "plain.md", "plain-agent"); // no `model:` field

        let cfg = AgentDiscoveryConfig {
            project_agent_dirs: vec![dir.path().to_path_buf()],
            ..AgentDiscoveryConfig::default()
        };

        let check = check_provider_catalog_freshness(None, &cfg).await;
        assert_eq!(check.status, CheckStatus::Ok);
        assert!(check.remedy.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_freshness_warns_when_overrides_exist_but_no_catalog_path_configured() {
        let dir = tempfile::tempdir().expect("real tempdir");
        write_agent_with_model(dir.path(), "opus.md", "opus-agent", "anthropic/claude-opus-4");

        let cfg = AgentDiscoveryConfig {
            project_agent_dirs: vec![dir.path().to_path_buf()],
            ..AgentDiscoveryConfig::default()
        };

        let check = check_provider_catalog_freshness(None, &cfg).await;
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.remedy.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_freshness_warns_when_configured_path_does_not_exist() {
        let dir = tempfile::tempdir().expect("real tempdir");
        write_agent_with_model(dir.path(), "opus.md", "opus-agent", "anthropic/claude-opus-4");
        let cfg = AgentDiscoveryConfig {
            project_agent_dirs: vec![dir.path().to_path_buf()],
            ..AgentDiscoveryConfig::default()
        };

        let missing_catalog = dir.path().join("catalog.json");
        let check = check_provider_catalog_freshness(Some(&missing_catalog), &cfg).await;
        assert_eq!(check.status, CheckStatus::Warn);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_freshness_ok_when_catalog_file_is_fresh() {
        let dir = tempfile::tempdir().expect("real tempdir");
        write_agent_with_model(dir.path(), "opus.md", "opus-agent", "anthropic/claude-opus-4");
        let cfg = AgentDiscoveryConfig {
            project_agent_dirs: vec![dir.path().to_path_buf()],
            ..AgentDiscoveryConfig::default()
        };

        let catalog_path = dir.path().join("catalog.json");
        tokio::fs::write(&catalog_path, b"{}")
            .await
            .expect("write fresh catalog file");

        let check = check_provider_catalog_freshness(Some(&catalog_path), &cfg).await;
        assert_eq!(check.status, CheckStatus::Ok, "{check:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_freshness_warns_when_catalog_file_is_stale() {
        let dir = tempfile::tempdir().expect("real tempdir");
        write_agent_with_model(dir.path(), "opus.md", "opus-agent", "anthropic/claude-opus-4");
        let cfg = AgentDiscoveryConfig {
            project_agent_dirs: vec![dir.path().to_path_buf()],
            ..AgentDiscoveryConfig::default()
        };

        let catalog_path = dir.path().join("catalog.json");
        tokio::fs::write(&catalog_path, b"{}")
            .await
            .expect("write catalog file");

        // Backdate the file's mtime well past the staleness threshold using `filetime`-free plain
        // std: `std::fs::File::set_modified` is stable and portable, no extra dependency needed.
        let stale_time =
            std::time::SystemTime::now() - (CATALOG_STALE_THRESHOLD + Duration::from_secs(3600));
        let file = std::fs::File::options()
            .write(true)
            .open(&catalog_path)
            .expect("reopen catalog file");
        file.set_modified(stale_time).expect("backdate mtime");
        drop(file);

        let check = check_provider_catalog_freshness(Some(&catalog_path), &cfg).await;
        assert_eq!(check.status, CheckStatus::Warn, "{check:?}");
        assert!(check.remedy.is_some());
    }

    // -----------------------------------------------------------------------------------------
    // humanize_duration
    // -----------------------------------------------------------------------------------------

    #[test]
    fn humanize_duration_renders_days_hours_minutes_seconds_by_magnitude() {
        assert_eq!(humanize_duration(Duration::from_secs(30)), "30s");
        assert_eq!(humanize_duration(Duration::from_secs(90)), "1m");
        assert_eq!(humanize_duration(Duration::from_secs(3661)), "1h 1m");
        assert_eq!(
            humanize_duration(Duration::from_secs(90_000)),
            "1d 1h"
        );
    }

    // -----------------------------------------------------------------------------------------
    // A-SA-16 (end-to-end): DoctorRunner::run against a deliberately misconfigured environment
    // reports Warn/Fail for EXACTLY the expected subset, Ok for the rest.
    // -----------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[cfg(unix)]
    async fn doctor_runner_reports_exactly_the_expected_warn_fail_subset_a_sa_16() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("real tempdir");

        // A healthy, discoverable agent (no model override) so (d) is Ok and (f) is Ok
        // ("nothing to check freshness against").
        write_agent(&dir.path().join("agents"), "scout.md", "scout");

        // Misconfiguration 1: config.json is simply absent (R-SA-131/A-SA-16's own fixture wording).
        let config_json_path = dir.path().join("config.json"); // never written

        // Misconfiguration 2: the async-root temp-scope directory is unwritable (chmod 0o500).
        let async_root = dir.path().join("unwritable-async-root");
        std::fs::create_dir_all(&async_root).expect("mkdir");
        std::fs::set_permissions(&async_root, std::fs::Permissions::from_mode(0o500))
            .expect("chmod read-only");

        let discovery_config = AgentDiscoveryConfig {
            project_agent_dirs: vec![dir.path().join("agents")],
            ..AgentDiscoveryConfig::default()
        };

        let runner = DoctorRunner {
            async_root: async_root.clone(),
            config_json_path,
            discovery_config,
            provider_catalog_path: None,
        };

        let report = runner.run().await;

        // Restore permissions so the tempdir guard can clean up regardless of assertion outcome.
        std::fs::set_permissions(&async_root, std::fs::Permissions::from_mode(0o700))
            .expect("restore permissions for cleanup");

        assert_eq!(
            report.checks.len(),
            6,
            "all six R-SA-131 checks must always be present in the report"
        );

        if running_as_root() {
            // Root bypasses the unwritable-directory fixture; skip the strict subset assertion in
            // that environment (still asserts the OTHER checks' behavior below).
            return;
        }

        // `CHECK_BINARY_RESOLUTION` is excluded from this strict-subset assertion: under `cargo
        // test`, `std::env::current_exe()` (R-SA-045 tier 2) resolves to the TEST HARNESS binary
        // itself, not a real `cyrup` build, so its `--version` probe legitimately exits non-zero
        // in this ambient test environment — an artifact of running under `cargo test`, not a
        // misconfiguration this test deliberately introduced. Its exact Ok/Warn/Fail behavior
        // against a controlled `SpawnCommand` is exhaustively covered by the dedicated
        // `binary_resolution_*` unit tests above instead; here it is asserted only to be present
        // in the report (below), with whatever status the ambient environment happens to produce.
        let actionable_names: std::collections::BTreeSet<&str> = report
            .actionable()
            .iter()
            .map(|c| c.name.as_str())
            .filter(|&name| name != CHECK_BINARY_RESOLUTION)
            .collect();

        assert_eq!(
            actionable_names,
            std::collections::BTreeSet::from([
                CHECK_TEMP_DIR_WRITABLE,
                CHECK_CONFIG_JSON,
            ]),
            "exactly the deliberately-misconfigured checks (temp-dir, config.json) must be \
             actionable; every other check (other than binary-resolution, excluded above for its \
             own documented reason) must report Ok. Full report: {report:#?}"
        );

        assert_eq!(
            report.find(CHECK_TEMP_DIR_WRITABLE).map(|c| c.status),
            Some(CheckStatus::Fail)
        );
        assert_eq!(
            report.find(CHECK_CONFIG_JSON).map(|c| c.status),
            Some(CheckStatus::Warn)
        );
        assert_eq!(
            report.find(CHECK_AGENT_DISCOVERY).map(|c| c.status),
            Some(CheckStatus::Ok)
        );
        assert_eq!(
            report.find(CHECK_CHAIN_DISCOVERY).map(|c| c.status),
            Some(CheckStatus::Ok)
        );
        assert_eq!(
            report
                .find(CHECK_PROVIDER_CATALOG_FRESHNESS)
                .map(|c| c.status),
            Some(CheckStatus::Ok)
        );
        // Binary resolution depends on the real ambient test environment's current_exe()/PATH
        // resolving to *something* that spawns; it is deliberately not part of THIS test's
        // misconfiguration and is asserted only to be present, not to any specific status, since
        // `cargo test`'s own current_exe() is the test binary itself, not a real `cyrup` — its
        // exact Ok/Warn outcome is exercised precisely by the dedicated
        // `binary_resolution_*` unit tests above instead.
        assert!(report.find(CHECK_BINARY_RESOLUTION).is_some());
    }
}
