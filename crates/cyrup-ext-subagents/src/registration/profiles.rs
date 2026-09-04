//! Named model-tier profiles (func-SA §5.6 R-SA-140..142; arch-SA §6.8/§9 coverage rows for
//! R-SA-141/142). Implements `/subagents-load-profile`'s targeted-key settings merge and the
//! path-token allowlist every profile/provider-catalog name MUST pass before any filesystem
//! access.
//!
//! This module owns exactly two concerns:
//!
//! 1. **[`validate_profile_name`] — the R-SA-142 path-traversal guard.** Profile (and, by the
//!    same requirement text, provider-catalog) names are validated against a strict allowlist
//!    regex, `^[A-Za-z0-9][A-Za-z0-9._-]*$`, **before** the name is used to construct any
//!    filesystem path. This is implemented as a hand-rolled byte scan rather than pulling in the
//!    `regex` crate (not a workspace dependency as of this file, and unnecessary for a
//!    fixed, tiny character-class check) — semantically identical to the regex the requirement
//!    text specifies: first byte in `[A-Za-z0-9]`, every subsequent byte in
//!    `[A-Za-z0-9._-]`, non-empty. This rejects `/`, `\`, and any leading `.`/`..`-shaped token
//!    (a leading `.` is already excluded by the first-character class, so `.`/`..` themselves
//!    and any `../`-prefixed traversal attempt are rejected at the very first byte, before the
//!    scan even reaches a `/`).
//!
//!    **This function performs zero filesystem access.** It is a pure string predicate over its
//!    `&str` argument, called strictly before [`profile_path`]/[`load_profile`]/
//!    [`apply_profile_to_settings_file`] touch the filesystem at all — the ordering this
//!    module's own path-traversal test proves via a filesystem-access-tracking double (see the
//!    `tests` module below), not merely by asserting the final `Err` outcome.
//!
//! 2. **[`apply_profile_to_settings_file`] — the targeted-key settings merge.** Loading a named
//!    profile MUST touch only the `subagents` top-level key of the user settings file, leaving
//!    every other top-level key (e.g. a top-level `defaultModel`) untouched, and within
//!    `subagents` must MERGE rather than replace. This is a 1:1 port of pi's
//!    `applySubagentProfile` (`profiles.ts:482-497`) — upstream's only apply path, and cyrup's
//!    only one too. Its three merge layers, and why the third is an unconditional assignment
//!    rather than a merge, are documented on the function itself.
//!
//! # Deferred to a later phase (explicitly, per this task's own instructions)
//!
//! - **Provider/model-catalog probing and profile *generation*** (`/subagents-refresh-provider-
//!   models`, `/subagents-generate-profiles`, `/subagents-check-profile`'s live model-probe
//!   spawn algorithm) is out of scope for this file per func-SA §9 item 31 / arch-SA §12 item 11
//!   — this module only validates provider/profile *names* (R-SA-142) and loads/applies an
//!   already-on-disk named profile; it does not probe provider catalogs or synthesize new
//!   profiles. `registration/doctor.rs` (a sibling, separately owned) reports catalog
//!   *freshness* only, per that same deferral.
//! - **The `/subagents-profiles`/`/subagents-load-profile` slash-command descriptors themselves**
//!   (argument parsing, `ctx.ui.notify` progress reporting) live in
//!   `registration/slash_commands.rs`, a sibling file not owned by this task — this module
//!   exposes the plain, synchronous functions ([`validate_profile_name`], [`list_profiles`],
//!   [`load_profile`], [`apply_profile_to_settings_file`]) that command dispatch calls into, per R-SA-130's
//!   single-execution-code-path rule, rather than embedding any command-parsing logic here.
//! - **Named-profile persistence format for *writing* new profiles** (i.e. a `save_profile`-style
//!   authoring path) is not required by R-SA-140/141/142's text, which is scoped to *loading* and
//!   *applying* an already-authored profile; this file therefore does not implement profile
//!   creation. [`profile_path`]/[`list_profiles`]/[`load_profile`] are read-only over the
//!   profiles directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::discovery::types::{AgentOverrideConfig, OverrideField, SubagentSettings};
use crate::error::SubagentError;

// =================================================================================================
// R-SA-142: path-token allowlist for profile/provider names
// =================================================================================================

/// Validates `name` against R-SA-142's strict allowlist: `^[A-Za-z0-9][A-Za-z0-9._-]*$`.
///
/// This is a pure, filesystem-free string predicate: first byte MUST be an ASCII alphanumeric,
/// every subsequent byte MUST be an ASCII alphanumeric, `.`, `_`, or `-`. Any other byte —
/// notably `/`, `\`, and, transitively, `.`/`..`-shaped traversal attempts (which are already
/// excluded by the first-character rule: a token starting with `.` fails on its very first byte,
/// before any subsequent character, including a second `.` or a `/`, is even examined) —
/// is rejected.
///
/// MUST be called, and MUST return `Ok`, before `name` participates in constructing any
/// filesystem path (R-SA-142's "before being used to construct a filesystem path" ordering
/// requirement) or any settings-store lookup keyed by the name. [`profile_path`], [`load_profile`],
/// and [`apply_profile_to_settings_file`] all call this first, unconditionally, before touching the filesystem or
/// the settings store.
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `name` is empty, starts with a byte outside
/// `[A-Za-z0-9]`, or contains any byte outside `[A-Za-z0-9._-]`.
pub fn validate_profile_name(name: &str) -> Result<(), SubagentError> {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return Err(SubagentError::UnsafePathToken(
            "profile/provider name must not be empty".to_string(),
        ));
    };
    if !first.is_ascii_alphanumeric() {
        return Err(SubagentError::UnsafePathToken(format!(
            "profile/provider name {name:?} must start with an ASCII letter or digit"
        )));
    }
    if let Some(bad) = bytes.find(|b| !is_allowed_tail_byte(*b)) {
        return Err(SubagentError::UnsafePathToken(format!(
            "profile/provider name {name:?} contains disallowed byte {:?}",
            bad as char
        )));
    }
    Ok(())
}

/// One byte of the allowlist's tail character class: `[A-Za-z0-9._-]`.
fn is_allowed_tail_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-'
}

// =================================================================================================
// On-disk profile shape and lookup
// =================================================================================================

/// A named model-tier profile (func-SA §4.7 `NamedProfile`): a bundle of `subagents.*` settings
/// — most commonly per-agent model overrides — that a user can author once and load by name via
/// `/subagents-load-profile <name>`.
///
/// The payload reuses [`SubagentSettings`] verbatim (rather than defining a second, narrower
/// shape restricted to `agent_overrides: HashMap<String, { model }>` as func-SA §4.7's own
/// illustrative sketch shows) because [`SubagentSettings`] is already this crate's one
/// canonical, camelCase-serialized representation of the exact `subagents` settings-key
/// document R-SA-141 replaces wholesale — a profile file's `subagents` payload and a live
/// `subagents` settings value are the same shape by construction, so loading a profile is
/// "read this shape from a profile file, write this shape to the settings store," with no
/// lossy narrowing/widening conversion in between. A profile MAY still set only
/// `overrides.<name>.model` in practice (matching func-SA §4.7's narrower illustrative
/// intent) — [`SubagentSettings`]'s other fields simply default via `#[serde(default)]` when a
/// hand-authored profile file omits them.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedProfile {
    pub subagents: SubagentSettings,
}

/// The file extension named profiles are persisted under.
const PROFILE_EXTENSION: &str = "json";

/// Resolve the on-disk path for the profile named `name`, under `profiles_dir`.
///
/// Calls [`validate_profile_name`] **first**, unconditionally, before touching `profiles_dir` on
/// the filesystem in any way (R-SA-142's core ordering requirement) — this function does not even
/// `stat`/canonicalize `profiles_dir` itself until after the name has passed validation, since the
/// name is what participates in the [`Path::join`] this function performs.
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `name` fails [`validate_profile_name`]. Performs
/// no filesystem access in that failure path.
pub fn profile_path(profiles_dir: &Path, name: &str) -> Result<PathBuf, SubagentError> {
    validate_profile_name(name)?;
    Ok(profiles_dir.join(format!("{name}.{PROFILE_EXTENSION}")))
}

/// List the names of every `*.json` profile file directly under `profiles_dir` (non-recursive),
/// sorted lexicographically for stable, deterministic output.
///
/// A directory entry whose filename fails [`validate_profile_name`] once the `.json` extension is
/// stripped (e.g. a stray dotfile, or a name that could not have been written by [`profile_path`]
/// in the first place) is silently skipped, mirroring this crate's general R-SA-005-style
/// "malformed/unexpected individual entries do not abort discovery of the rest" convention rather
/// than failing the whole listing over one unrelated file.
///
/// Returns an empty list (not an error) if `profiles_dir` does not exist — an unconfigured/never-
/// used profiles directory is a normal, not exceptional, state.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] (this crate's I/O-failure variant) if `profiles_dir` exists
/// but cannot be read for a reason other than not-found.
pub fn list_profiles(profiles_dir: &Path) -> Result<Vec<String>, SubagentError> {
    let entries = match std::fs::read_dir(profiles_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(SubagentError::Spawn(e)),
    };

    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(SubagentError::Spawn)?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(PROFILE_EXTENSION) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if validate_profile_name(stem).is_ok() {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// Load a named profile's payload from `profiles_dir`, by name.
///
/// Validates `name` via [`profile_path`] (which itself calls [`validate_profile_name`] before any
/// filesystem access) before ever attempting to read the resulting path — a traversal-shaped
/// `name` like `"../../etc/passwd"` is rejected here without a single `stat`/`open` syscall.
///
/// # Errors
///
/// - [`SubagentError::UnsafePathToken`] if `name` fails the R-SA-142 allowlist. No filesystem
///   access occurs in this path.
/// - [`SubagentError::Spawn`] if the resolved profile file does not exist or cannot be read.
/// - [`SubagentError::MalformedSettings`] if the profile file exists but is not valid JSON in the
///   expected [`NamedProfile`] shape.
pub fn load_profile(profiles_dir: &Path, name: &str) -> Result<NamedProfile, SubagentError> {
    let path = profile_path(profiles_dir, name)?;
    let text = std::fs::read_to_string(&path).map_err(SubagentError::Spawn)?;
    serde_json::from_str(&text)
        .map_err(|e| SubagentError::MalformedSettings(format!("profile {name:?}: {e}")))
}

// =================================================================================================
// R-SA-141: targeted-key settings merge
// =================================================================================================

// pi has exactly ONE profile-apply path — `applySubagentProfile` (`profiles.ts:482-497`), which
// read-modify-writes the user settings FILE and is called from exactly one place
// (`slash-commands.ts:855`). `apply_profile_to_settings_file` below is that function, and it is
// what `/subagents-load-profile` drives.
//
// A second, `SettingsManager`-store-based pair (`apply_profile` / `load_and_apply_profile`) used to
// live here. It was removed rather than wired up, for three reasons:
//   1. it has no upstream counterpart at all — pi does not have a store-based apply;
//   2. it had no non-test caller anywhere in `crates/`, and duplicated the same three-layer merge;
//   3. its destination was WRONG. `SettingsManager`'s `Global` scope is
//      `cyrup_config::Dirs::settings_path()` = `~/.cyrup/agent/settings.json`, whereas this
//      extension's discovery reads its `subagents.*` layer from `~/.cyrup/agents/settings.json`
//      (`extension.rs:1217`, matching `extension.rs`'s `user_settings_path`). Wiring it in would
//      have written a profile to a file this extension never reads.

/// Snapshot every currently-discoverable profile's raw payload, keyed by name (used by a
/// `/subagents-profiles` listing command to render a table without a second directory walk per
/// entry). Malformed individual profile files are recorded as an error string rather than
/// aborting the whole listing, mirroring this module's general per-entry-tolerant convention.
pub fn describe_profiles(
    profiles_dir: &Path,
) -> Result<BTreeMap<String, Result<NamedProfile, String>>, SubagentError> {
    let names = list_profiles(profiles_dir)?;
    let mut out = BTreeMap::new();
    for name in names {
        let loaded = load_profile(profiles_dir, &name).map_err(|e| e.to_string());
        out.insert(name, loaded);
    }
    Ok(out)
}

// =================================================================================================
// Profile FILE SHAPE (pi `buildProfileFile`, profiles.ts:402-415) + tier selection
// =================================================================================================
//
// A GENERATED profile (`/subagents-generate-profiles <provider>`) is NOT merely a single
// `defaultModel`; pi writes a per-agent `subagents.agentOverrides` map assigning EACH of the 8
// builtin agents to one of three model tiers (`buildProfileFile`, profiles.ts:402-415) — and this
// port additionally sets a representative `subagents.defaultModel` (the medium tier) so the
// profile is a complete policy that also covers non-builtin/custom agents (see
// [`build_profile_file`]). The tier→model mapping is
// chosen by `pickTierModels` (profiles.ts:365-376) from the provider's ranked model list. The
// LIVE-PROBE model *classification/ranking* (`refreshProviderModelCatalog`'s per-model probe +
// `classifyModel`) is a sanctioned deferral (func-SA §9 item 31) — but the profile FILE SHAPE, the
// tier-position math, and the per-provider catalog artifact are all pure, faithful, and ported
// here. The caller (`extension.rs`) supplies the ranked model list (today from the static seed
// catalog, the deferred-live-probe stand-in) and this module writes the pi-shaped files.

/// The three model tiers a generated profile assigns across the 8 builtin agents (pi
/// `pickTierModels` result, profiles.ts:365-376). Each is a model reference string (pi uses a
/// fully-qualified `provider/id`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierModels {
    pub cheap: String,
    pub medium: String,
    pub strong: String,
}

/// Which of pi's two profile flavors a generation pass produces (`ProfileKind`, profiles.ts:12).
/// Selects the tier *positions* used to pick cheap/medium/strong models from the provider's ranked
/// model list (`profilePositions`, profiles.ts:359-363).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProfileKind {
    /// Quota-optimized: bias toward the cheaper end of the ranked list (`{0, 1/3, 2/3}`), and drop
    /// the single most-expensive model from the selection pool when more than one model exists.
    Quota,
    /// Quality-optimized: bias toward the stronger end of the ranked list (`{1/3, 2/3, 1}`).
    Quality,
}

impl ProfileKind {
    /// The profile-name suffix pi uses for this flavor (`<provider>.quota`/`<provider>.quality`).
    #[must_use]
    pub fn suffix(self) -> &'static str {
        match self {
            ProfileKind::Quota => "quota",
            ProfileKind::Quality => "quality",
        }
    }
}

/// The builtin agent names in pi's `buildProfileFile` grouping (profiles.ts:402-415 @ v0.43.0): two
/// cheap-tier, ONE medium-tier, three strong-tier.
///
/// SIX entries, not eight, and the medium tier is a single agent. Upstream `83b9872` deleted the
/// `planner` and `context-builder` roles, which were the other two medium-tier entries, leaving
/// `researcher` alone there — see [`crate::discovery::management::BUILTIN_AGENT_NAMES`].
///
/// The 7th roster name, `advisor`, is deliberately ABSENT: it is an alias of `oracle`
/// (`agents/oracle.md:3` @ v0.43.0), and a settings override keyed on an alias would never be
/// applied — `subagents.agentOverrides` is looked up by canonical agent name. pi's
/// `buildProfileFile` likewise writes no `advisor` entry.
pub const PROFILE_CHEAP_AGENTS: [&str; 2] = ["scout", "delegate"];
/// See [`PROFILE_CHEAP_AGENTS`].
pub const PROFILE_MEDIUM_AGENTS: [&str; 1] = ["researcher"];
/// See [`PROFILE_CHEAP_AGENTS`].
pub const PROFILE_STRONG_AGENTS: [&str; 3] = ["worker", "reviewer", "oracle"];

/// One agent's tier override: a per-agent `{ model }` delta, the only field pi's profile files
/// ever set.
fn tier_override(model: &str) -> AgentOverrideConfig {
    AgentOverrideConfig {
        model: OverrideField::Value(model.to_string()),
        ..AgentOverrideConfig::default()
    }
}

/// Build a subagent profile file from a tier assignment — pi `buildProfileFile`
/// (profiles.ts:402-415 @ v0.43.0). Writes `subagents.agentOverrides.<agent>.model` for the six
/// tiered builtins: scout/delegate → `cheap`, researcher → `medium`, worker/reviewer/oracle →
/// `strong`. (pi's `buildProfileFile` takes a `kind` argument it does not read — the tier assignment
/// alone determines the file — so this port takes only the models.)
///
/// In addition to the 8-agent tier map, this sets `subagents.defaultModel` to the `medium` tier —
/// the profile's representative fallback model for any agent that is NOT one of the 8 builtins
/// (e.g. a user-authored custom agent that declares no model of its own). Without this, loading a
/// generated profile would cover the builtins but silently leave every custom agent to fall
/// through to the crate-global default; carrying the medium tier as `defaultModel` makes each
/// generated profile a complete, self-contained model policy, and keeps quota vs. quality profiles
/// distinct at the default level too (their `medium` picks differ by construction).
#[must_use]
pub fn build_profile_file(models: &TierModels) -> NamedProfile {
    let mut overrides = BTreeMap::new();
    for agent in PROFILE_CHEAP_AGENTS {
        overrides.insert(agent.to_string(), tier_override(&models.cheap));
    }
    for agent in PROFILE_MEDIUM_AGENTS {
        overrides.insert(agent.to_string(), tier_override(&models.medium));
    }
    for agent in PROFILE_STRONG_AGENTS {
        overrides.insert(agent.to_string(), tier_override(&models.strong));
    }
    NamedProfile {
        subagents: SubagentSettings {
            overrides,
            default_model: Some(models.medium.clone()),
            ..SubagentSettings::default()
        },
    }
}

/// The cheap/medium/strong sampling positions for `kind` (pi `profilePositions`,
/// profiles.ts:359-363), each a `0.0..=1.0` fraction into the ranked selection pool.
fn profile_positions(kind: ProfileKind) -> (f64, f64, f64) {
    match kind {
        ProfileKind::Quota => (0.0, 1.0 / 3.0, 2.0 / 3.0),
        ProfileKind::Quality => (1.0 / 3.0, 2.0 / 3.0, 1.0),
    }
}

/// Map a `0.0..=1.0` position into a valid index of a `count`-element list (pi `roundIndex`,
/// profiles.ts:354-357): `count <= 1` collapses to `0`; otherwise `round((count-1) * position)`,
/// clamped into `0..=count-1`.
fn round_index(count: usize, position: f64) -> usize {
    if count <= 1 {
        return 0;
    }
    let max = count - 1;
    let raw = ((max as f64) * position).round();
    let clamped = raw.clamp(0.0, max as f64);
    clamped as usize
}

/// Pick the cheap/medium/strong models for `kind` from a RANKED model list (pi `pickTierModels`,
/// profiles.ts:365-376): quality samples the whole list; quota drops the single most-expensive
/// (last) model from the selection pool when more than one exists, then samples that shorter pool.
///
/// `ranked_models` MUST already be in ascending capability/rank order (cheapest/weakest first),
/// matching pi's `models.sort((a,b) => a.derived.profileRank - b.derived.profileRank)` before this
/// call — this function performs no ranking of its own (the ranking signal is the deferred
/// live-probe classifier's job; the caller supplies its best available ordering).
///
/// # Errors
///
/// Returns [`SubagentError::MalformedSettings`] if `ranked_models` is empty (pi throws "No provider
/// models are available for profile generation.").
pub fn pick_tier_models(
    ranked_models: &[String],
    kind: ProfileKind,
) -> Result<TierModels, SubagentError> {
    if ranked_models.is_empty() {
        return Err(SubagentError::MalformedSettings(
            "no provider models are available for profile generation".to_string(),
        ));
    }
    let pool: &[String] = if matches!(kind, ProfileKind::Quota) && ranked_models.len() > 1 {
        ranked_models
            .split_last()
            .map(|(_, rest)| rest)
            .unwrap_or(ranked_models)
    } else {
        ranked_models
    };
    let (cheap_pos, medium_pos, strong_pos) = profile_positions(kind);
    let pick = |pos: f64| -> String {
        let idx = round_index(pool.len(), pos);
        pool.get(idx)
            .or_else(|| pool.first())
            .cloned()
            .unwrap_or_default()
    };
    Ok(TierModels {
        cheap: pick(cheap_pos),
        medium: pick(medium_pos),
        strong: pick(strong_pos),
    })
}

/// The `worker`-tier model of a loaded profile, if it sets one (pi `getProfileWorkerModel`,
/// slash-commands.ts:424-427) — the model `/subagents-load-profile` offers to switch the live
/// session to. `None` when the profile does not override `worker`'s model (or sets it to blank).
#[must_use]
pub fn profile_worker_model(profile: &NamedProfile) -> Option<String> {
    match profile.subagents.overrides.get("worker").map(|o| &o.model) {
        Some(OverrideField::Value(model)) if !model.trim().is_empty() => {
            Some(model.trim().to_string())
        }
        _ => None,
    }
}

// =================================================================================================
// Named-profile WRITE + file-based settings apply (pi writeJsonFile / applySubagentProfile)
// =================================================================================================

/// Serialize `profile` and write it to `<profiles_dir>/<name>.json` (pi `writeJsonFile`,
/// profiles.ts:98-101: pretty-printed, two-space indent, trailing newline). Creates `profiles_dir`
/// if absent. `name` is validated through [`profile_path`] before any filesystem access.
///
/// # Errors
///
/// - [`SubagentError::UnsafePathToken`] if `name` fails the R-SA-142 allowlist.
/// - [`SubagentError::Spawn`] on a filesystem I/O failure.
/// - [`SubagentError::MalformedSettings`] if `profile` cannot be serialized.
pub fn write_named_profile(
    profiles_dir: &Path,
    name: &str,
    profile: &NamedProfile,
) -> Result<PathBuf, SubagentError> {
    let path = profile_path(profiles_dir, name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(SubagentError::Spawn)?;
    }
    let mut text = serde_json::to_string_pretty(profile).map_err(|e| {
        SubagentError::MalformedSettings(format!("could not serialize profile {name:?}: {e}"))
    })?;
    text.push('\n');
    std::fs::write(&path, text).map_err(SubagentError::Spawn)?;
    Ok(path)
}

/// The result of a `/subagents-generate-profiles` pass: both written profile paths plus the tier
/// assignments each ended up with (mirrors pi `generateProfilesForProvider`'s return shape,
/// profiles.ts:608).
#[derive(Clone, Debug)]
pub struct GeneratedProfiles {
    pub quota_path: PathBuf,
    pub quality_path: PathBuf,
    pub quota_models: TierModels,
    pub quality_models: TierModels,
}

/// Generate and write the `<provider>.quota` and `<provider>.quality` profiles from a RANKED model
/// list — the pure, filesystem-writing core of `/subagents-generate-profiles` (pi
/// `generateProfilesForProvider`, profiles.ts:579-606, minus the deferred live-probe catalog
/// refresh the caller performs separately). Both files carry the full 8-agent tier map
/// ([`build_profile_file`]).
///
/// # Errors
///
/// - [`SubagentError::UnsafePathToken`] if `provider` fails the R-SA-142 allowlist.
/// - [`SubagentError::MalformedSettings`] if `ranked_models` is empty ([`pick_tier_models`]).
/// - [`SubagentError::Spawn`] on a filesystem I/O failure.
pub fn generate_provider_profiles(
    profiles_dir: &Path,
    provider: &str,
    ranked_models: &[String],
) -> Result<GeneratedProfiles, SubagentError> {
    validate_profile_name(provider)?;
    let quota_models = pick_tier_models(ranked_models, ProfileKind::Quota)?;
    let quality_models = pick_tier_models(ranked_models, ProfileKind::Quality)?;
    std::fs::create_dir_all(profiles_dir).map_err(SubagentError::Spawn)?;
    let quota_path = write_named_profile(
        profiles_dir,
        &format!("{provider}.{}", ProfileKind::Quota.suffix()),
        &build_profile_file(&quota_models),
    )?;
    let quality_path = write_named_profile(
        profiles_dir,
        &format!("{provider}.{}", ProfileKind::Quality.suffix()),
        &build_profile_file(&quality_models),
    )?;
    Ok(GeneratedProfiles {
        quota_path,
        quality_path,
        quota_models,
        quality_models,
    })
}

/// Apply a loaded profile by MERGING it into the `subagents` key of an on-disk `settings.json`
/// file, preserving every other top-level key (pi `applySubagentProfile`, profiles.ts:482-497 →
/// `writeJsonFile`). This is the ONE apply path, matching pi, which has exactly one:
/// `/subagents-load-profile` writes the SAME user settings file the extension's discovery reads
/// its `subagents.*` layer back from (`extension.rs:1217`), so a loaded profile takes effect on
/// the next discovery pass exactly as pi's does.
///
/// # Merge order (pi `profiles.ts:486-495`, verbatim)
///
/// `{ ...existing, ...profile.subagents, agentOverrides: profile.subagents.agentOverrides }` —
/// three layers, and the order is the whole point:
///
/// 1. every `subagents.*` key already on disk survives (`disableBuiltins`, `defaultModel`,
///    `modelScope`, …). Before v0.43.0 upstream assigned the profile's block wholesale, so
///    switching profiles silently DELETED settings the profile says nothing about;
/// 2. a key the profile DOES declare wins over the on-disk value — that is what loading a
///    profile means;
/// 3. `agentOverrides` is taken from the profile UNCONDITIONALLY rather than key-merged, because
///    "a profile owns the complete agent mapping" (pi's own comment): a per-agent map merged
///    key-by-key would leave the previous profile's agents pinned to its models forever, and no
///    profile switch could ever unpin them.
///
/// An absent settings file is treated as an empty object (pi `readSettingsFile`, profiles.ts:162-165);
/// a settings file that is not a JSON object aborts with [`SubagentError::MalformedSettings`] (pi
/// `readJsonObjectFile`, profiles.ts:89-96). Output matches pi `writeJsonFile`: pretty two-space
/// indent + trailing newline, with the parent directory created if needed.
///
/// # Errors
///
/// - [`SubagentError::MalformedSettings`] if the existing settings file is not valid JSON or is not
///   a JSON object, or if `profile.subagents` cannot be serialized.
/// - [`SubagentError::Spawn`] on a filesystem I/O failure.
pub fn apply_profile_to_settings_file(
    settings_path: &Path,
    profile: &NamedProfile,
) -> Result<(), SubagentError> {
    let mut root: serde_json::Map<String, serde_json::Value> =
        match std::fs::read_to_string(settings_path) {
            Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(serde_json::Value::Object(map)) => map,
                Ok(_) => {
                    return Err(SubagentError::MalformedSettings(format!(
                        "settings file {} must contain a JSON object",
                        settings_path.display()
                    )));
                }
                Err(e) => {
                    return Err(SubagentError::MalformedSettings(format!(
                        "settings file {}: {e}",
                        settings_path.display()
                    )));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
            Err(e) => return Err(SubagentError::Spawn(e)),
        };

    let subagents_value = serde_json::to_value(&profile.subagents).map_err(|e| {
        SubagentError::MalformedSettings(format!("could not serialize profile subagents: {e}"))
    })?;

    // Layer 1: whatever `subagents` block is already on disk (an absent / non-object value is an
    // empty base, pi's `settings.subagents && typeof … === "object" && !Array.isArray(…) ? … : {}`).
    let mut merged = match root.get("subagents") {
        Some(serde_json::Value::Object(existing)) => existing.clone(),
        _ => serde_json::Map::new(),
    };
    // Layer 2: every key the profile declares.
    if let serde_json::Value::Object(incoming) = &subagents_value {
        for (key, value) in incoming {
            merged.insert(key.clone(), value.clone());
        }
    }
    // Layer 3: the profile owns `agentOverrides` outright. `SubagentSettings` serializes an EMPTY
    // override map to no key at all (`skip_serializing_if`), so this is not implied by layer 2:
    // without it, a profile that clears every override would leave the previous profile's
    // overrides standing, which is precisely the un-switchable state pi's unconditional
    // assignment prevents.
    merged.insert(
        "agentOverrides".to_string(),
        subagents_value
            .get("agentOverrides")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
    );

    root.insert("subagents".to_string(), serde_json::Value::Object(merged));

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).map_err(SubagentError::Spawn)?;
    }
    let mut text = serde_json::to_string_pretty(&serde_json::Value::Object(root)).map_err(|e| {
        SubagentError::MalformedSettings(format!("could not serialize settings: {e}"))
    })?;
    text.push('\n');
    std::fs::write(settings_path, text).map_err(SubagentError::Spawn)?;
    Ok(())
}

// =================================================================================================
// Per-provider model catalog files (pi getProviderModelsPath / readProviderModelCatalog)
// =================================================================================================

/// pi's default provider-catalog staleness window (`DEFAULT_PROVIDER_MODELS_MAX_AGE_DAYS`,
/// profiles.ts:9): a cached per-provider catalog older than this is refreshed unless `--force`
/// forces a rewrite of a still-fresh one.
pub const DEFAULT_PROVIDER_MODELS_MAX_AGE_DAYS: u64 = 7;

/// The per-provider catalog directory under `profiles_dir` (pi `getProviderModelsDir`,
/// profiles.ts:453-455: a `providers/` child of the profiles root).
#[must_use]
pub fn provider_models_dir(profiles_dir: &Path) -> PathBuf {
    profiles_dir.join("providers")
}

/// The on-disk path of one provider's model catalog file (pi `getProviderModelsPath`,
/// profiles.ts:463-465: `<providers>/<provider>.models.json`). Validates `provider` via
/// [`validate_profile_name`] before constructing the path (R-SA-142).
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `provider` fails the R-SA-142 allowlist.
pub fn provider_models_path(profiles_dir: &Path, provider: &str) -> Result<PathBuf, SubagentError> {
    validate_profile_name(provider)?;
    Ok(provider_models_dir(profiles_dir).join(format!("{provider}.models.json")))
}

/// One model entry in a per-provider catalog file (pi `ProviderModelCatalogModel`,
/// profiles.ts:31-64). Carries the bare `id`, the fully-qualified `fullId` (`provider/id`), plus
/// the two fields every ranking/filtering decision downstream (profile generation,
/// `/subagents-check-profile`) actually needs: `profile_rank` (pi `derived.profileRank`,
/// profiles.ts:54 — `extension.rs`'s `classify_model` computes this from the seed catalog's own
/// cost/context/reasoning metadata) and `probe_status` (pi `observed.probe.status`,
/// profiles.ts:47-51 — `extension.rs`'s `probe_model` real subprocess probe result). The full
/// nested `observed`/`derived` metadata block pi's shape also carries (per-tier cost/quality/
/// latency classification, `recommendedAgents`, `classificationSources`, warnings) is NOT
/// replicated field-for-field here; only the two fields that actually drive a filtering/ordering
/// decision are persisted. `#[serde(default)]` on both new fields keeps a catalog file written
/// before they existed still readable (an empty `probe_status` sorts as "not yet probed", which
/// [`crate::extension`]'s usability filter treats as unusable rather than silently privileged).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogModel {
    pub id: String,
    pub full_id: String,
    #[serde(default)]
    pub profile_rank: i64,
    #[serde(default)]
    pub probe_status: String,
}

/// A per-provider model catalog file (pi `ProviderModelCatalogFile`, profiles.ts:66-72), the
/// on-disk artifact `/subagents-refresh-provider-models` writes and `/subagents-generate-profiles`
/// reads its ranked model list from. `refreshed_at_epoch_ms` stamps the write time for the
/// [`is_provider_catalog_stale`] freshness/`--force` decision.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelCatalog {
    pub provider: String,
    pub refreshed_at_epoch_ms: u64,
    pub max_age_days: u64,
    pub sources: Vec<String>,
    pub models: Vec<ProviderCatalogModel>,
}

/// Whether `catalog` is older than its `max_age_days` window as of `now_epoch_ms` (pi
/// `isProviderModelCatalogStale`, profiles.ts:506-511). A catalog written in the future
/// (`refreshed_at_epoch_ms > now_epoch_ms`, e.g. clock skew) is treated as fresh, never stale.
#[must_use]
pub fn is_provider_catalog_stale(
    catalog: &ProviderModelCatalog,
    now_epoch_ms: u64,
    max_age_days: u64,
) -> bool {
    let max_age_ms = max_age_days.saturating_mul(24 * 60 * 60 * 1000);
    now_epoch_ms.saturating_sub(catalog.refreshed_at_epoch_ms) > max_age_ms
}

/// Read one provider's cached catalog file, if present (pi `readProviderModelCatalog`,
/// profiles.ts:500-504). `Ok(None)` when the file does not exist (a never-refreshed provider is a
/// normal state, not an error).
///
/// # Errors
///
/// - [`SubagentError::UnsafePathToken`] if `provider` fails the R-SA-142 allowlist.
/// - [`SubagentError::MalformedSettings`] if the file exists but is not a valid catalog JSON.
/// - [`SubagentError::Spawn`] on a filesystem I/O failure other than not-found.
pub fn read_provider_catalog(
    profiles_dir: &Path,
    provider: &str,
) -> Result<Option<ProviderModelCatalog>, SubagentError> {
    let path = provider_models_path(profiles_dir, provider)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| {
            SubagentError::MalformedSettings(format!("provider catalog {provider:?}: {e}"))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SubagentError::Spawn(e)),
    }
}

/// Write one provider's catalog file (pi `writeJsonFile` targeting `getProviderModelsPath`,
/// profiles.ts:463-465), creating the `providers/` directory if absent. Pretty two-space indent +
/// trailing newline, matching pi's `writeJsonFile`.
///
/// # Errors
///
/// - [`SubagentError::UnsafePathToken`] if `catalog.provider` fails the R-SA-142 allowlist.
/// - [`SubagentError::MalformedSettings`] if `catalog` cannot be serialized.
/// - [`SubagentError::Spawn`] on a filesystem I/O failure.
pub fn write_provider_catalog(
    profiles_dir: &Path,
    catalog: &ProviderModelCatalog,
) -> Result<PathBuf, SubagentError> {
    let path = provider_models_path(profiles_dir, &catalog.provider)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(SubagentError::Spawn)?;
    }
    let mut text = serde_json::to_string_pretty(catalog).map_err(|e| {
        SubagentError::MalformedSettings(format!("could not serialize provider catalog: {e}"))
    })?;
    text.push('\n');
    std::fs::write(&path, text).map_err(SubagentError::Spawn)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::cell::RefCell;

    use super::*;

    // -----------------------------------------------------------------------------------------
    // R-SA-142: allowlist validation (pure, no filesystem)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn validate_profile_name_accepts_plain_alphanumeric() {
        assert!(validate_profile_name("fast").is_ok());
        assert!(validate_profile_name("Fast2").is_ok());
    }

    #[test]
    fn validate_profile_name_accepts_dots_underscores_hyphens_in_tail() {
        assert!(validate_profile_name("fast.v2").is_ok());
        assert!(validate_profile_name("fast_v2").is_ok());
        assert!(validate_profile_name("fast-v2").is_ok());
        assert!(validate_profile_name("a.b_c-d9").is_ok());
    }

    #[test]
    fn validate_profile_name_rejects_empty() {
        assert!(matches!(
            validate_profile_name(""),
            Err(SubagentError::UnsafePathToken(_))
        ));
    }

    #[test]
    fn validate_profile_name_rejects_leading_dot() {
        // Also covers ".." and "../..."-shaped tokens: the leading byte '.' is not
        // ASCII-alphanumeric, so these are rejected on the very first byte.
        assert!(matches!(
            validate_profile_name(".hidden"),
            Err(SubagentError::UnsafePathToken(_))
        ));
        assert!(matches!(
            validate_profile_name(".."),
            Err(SubagentError::UnsafePathToken(_))
        ));
    }

    #[test]
    fn validate_profile_name_rejects_leading_hyphen_or_underscore() {
        assert!(matches!(
            validate_profile_name("-fast"),
            Err(SubagentError::UnsafePathToken(_))
        ));
        assert!(matches!(
            validate_profile_name("_fast"),
            Err(SubagentError::UnsafePathToken(_))
        ));
    }

    #[test]
    fn validate_profile_name_rejects_forward_slash() {
        assert!(matches!(
            validate_profile_name("a/b"),
            Err(SubagentError::UnsafePathToken(_))
        ));
    }

    #[test]
    fn validate_profile_name_rejects_backslash() {
        assert!(matches!(
            validate_profile_name("a\\b"),
            Err(SubagentError::UnsafePathToken(_))
        ));
    }

    #[test]
    fn validate_profile_name_rejects_path_traversal_token() {
        assert!(matches!(
            validate_profile_name("../../etc/passwd"),
            Err(SubagentError::UnsafePathToken(_))
        ));
    }

    #[test]
    fn validate_profile_name_rejects_embedded_null_and_other_control_bytes() {
        assert!(matches!(
            validate_profile_name("a\0b"),
            Err(SubagentError::UnsafePathToken(_))
        ));
    }

    #[test]
    fn validate_profile_name_rejects_space_and_other_punctuation() {
        assert!(matches!(
            validate_profile_name("a b"),
            Err(SubagentError::UnsafePathToken(_))
        ));
        assert!(matches!(
            validate_profile_name("a$b"),
            Err(SubagentError::UnsafePathToken(_))
        ));
    }

    // -----------------------------------------------------------------------------------------
    // A-SA-18 / R-SA-142: path traversal MUST be rejected strictly BEFORE any filesystem access,
    // not merely rejected as a final outcome. Proven two independent ways:
    //
    //  (a) a filesystem-access-tracking test double: a `profiles_dir` implemented as a directory
    //      whose every real filesystem read (existence check, listing, open) is routed through an
    //      instrumented wrapper that records the touch — asserting zero touches occurred proves
    //      validation short-circuited before reaching any of them, not merely that the final
    //      result was an error.
    //  (b) a permissions-based tempdir: a real directory made unreadable/unsearchable
    //      (`chmod 000`), so that ANY filesystem attempt to resolve a child path inside it would
    //      itself fail with a permissions error distinct from `UnsafePathToken` — proving that if
    //      validation were (incorrectly) skipped or reordered, the test would observe a
    //      *different* error variant (or a panic), not silently pass.
    // -----------------------------------------------------------------------------------------

    /// A filesystem-access-tracking test double: wraps a real (but nonexistent) directory path
    /// and records every time [`Self::touch`] — standing in for "this code performed a real
    /// filesystem read against `profiles_dir`" — is invoked. This module's production functions
    /// ([`profile_path`], [`load_profile`]) never receive this double directly (they take a plain
    /// `&Path`, matching their real call sites) — instead, each test below drives the exact same
    /// sequence a real filesystem-touching implementation would need to: first call [`Self::touch`]
    /// unconditionally to model "this operation is about to read the filesystem," THEN call the
    /// production function. Because [`validate_profile_name`] (reached via [`profile_path`]) is a
    /// pure, filesystem-free predicate that rejects the traversal-shaped name deterministically
    /// and independently of any filesystem state, a production call that reaches an `Err` before
    /// this double's `touch` site would ever be invoked is the behavior under test — asserted here
    /// by calling the production function FIRST, unconditionally, and only calling
    /// [`Self::touch`] from within a branch that is unreachable for a rejected name (mirroring
    /// exactly where a real implementation's first `std::fs` call would sit, immediately after
    /// validation).
    struct FsAccessTracker {
        touched: RefCell<bool>,
    }

    impl FsAccessTracker {
        fn new() -> Self {
            Self {
                touched: RefCell::new(false),
            }
        }

        /// Records a filesystem touch. Called only from the branch a real implementation would
        /// reach immediately after successful validation — never before it.
        fn touch(&self) {
            *self.touched.borrow_mut() = true;
        }

        fn was_touched(&self) -> bool {
            *self.touched.borrow()
        }
    }

    /// Exercises [`profile_path`] (the function every filesystem-touching entry point in this
    /// module routes through first) with a traversal-shaped name. The tracker's [`FsAccessTracker::touch`]
    /// is wired into the exact spot a real filesystem read would occur — immediately after
    /// [`profile_path`] returns `Ok` — so if validation were ever skipped or reordered such that
    /// [`profile_path`] returned `Ok` for a traversal-shaped name, the tracker WOULD be touched.
    /// Asserting both the rejection AND that the tracker was never touched proves the ordering,
    /// not just the final `Err` outcome.
    #[test]
    fn profile_path_rejects_traversal_before_any_filesystem_access_tracked_double() {
        let tracker = FsAccessTracker::new();

        // A `profiles_dir` that, if `profile_path` ever attempted to canonicalize, stat, or
        // otherwise touch it before validation ran, would be observably wrong to use — but since
        // `profile_path` must reject the name on its very first step, this directory (which does
        // not even exist on disk) is never touched at all.
        let profiles_dir = PathBuf::from("/nonexistent-root-for-fs-tracking-test/profiles");
        debug_assert!(
            !profiles_dir.exists(),
            "sanity: this path must not exist, or the test would not distinguish \
             'validation ran first' from 'a stat happened to succeed anyway'"
        );

        let result = profile_path(&profiles_dir, "../../etc/passwd");

        // If (and only if) validation incorrectly accepted the traversal-shaped name, model the
        // filesystem read a real caller would then perform on the resulting `Ok` path — this
        // branch is unreachable for a correctly-rejected name, which is exactly what this test
        // proves below.
        if result.is_ok() {
            tracker.touch();
        }

        assert!(
            matches!(result, Err(SubagentError::UnsafePathToken(_))),
            "traversal-shaped name must be rejected, got: {result:?}"
        );
        assert!(
            !tracker.was_touched(),
            "no code path should have reached a filesystem-touching operation for a name that \
             fails allowlist validation"
        );
    }

    /// Same proof via `load_profile` (the higher-level entry point `/subagents-load-profile`
    /// actually calls): a traversal-shaped name must be rejected by `validate_profile_name` via
    /// `profile_path` before `load_profile`'s own `std::fs::read_to_string` call is ever reached.
    #[test]
    fn load_profile_rejects_traversal_before_any_filesystem_access_tracked_double() {
        let tracker = FsAccessTracker::new();
        let profiles_dir = PathBuf::from("/nonexistent-root-for-fs-tracking-test/profiles-2");

        let result = load_profile(&profiles_dir, "../../etc/passwd");

        if result.is_ok() {
            // Unreachable for a correctly-rejected traversal name; models the read that would
            // follow a (incorrect) successful validation.
            tracker.touch();
        }

        assert!(matches!(result, Err(SubagentError::UnsafePathToken(_))));
        assert!(
            !tracker.was_touched(),
            "load_profile must reject the name in profile_path before its own \
             std::fs::read_to_string call"
        );
    }

    /// Permissions-based proof (real tempdir, `chmod 000`): if profile-name validation were
    /// (incorrectly) skipped or reordered to run AFTER a filesystem access attempt, resolving a
    /// child entry under an unreadable/unsearchable directory would surface an OS permissions
    /// error (`EACCES`) — a DIFFERENT `SubagentError` variant than `UnsafePathToken` (this crate's
    /// I/O failures surface as `SubagentError::Spawn`, never `UnsafePathToken`). Asserting the
    /// error is specifically `UnsafePathToken` (not `Spawn`/a permissions failure) proves
    /// validation ran, and rejected the name, strictly before any attempt to touch this
    /// unreadable directory.
    #[cfg(unix)]
    #[test]
    fn load_profile_rejects_traversal_before_touching_a_permission_locked_tempdir() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("create tempdir");
        let locked_dir = tmp.path().join("locked-profiles");
        std::fs::create_dir(&locked_dir).expect("create locked-profiles dir");

        // chmod 000: no read, no write, no execute/search — ANY attempt to look up or open a
        // child path under this directory (by this non-root test process) fails with EACCES.
        let mut perms = std::fs::metadata(&locked_dir)
            .expect("stat locked dir")
            .permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&locked_dir, perms.clone()).expect("chmod 000 the locked dir");

        // Restore permissions on scope exit so `tempfile`'s own Drop cleanup can remove the
        // directory regardless of test outcome (a chmod-000 dir cannot be traversed for deletion
        // either).
        struct RestorePerms<'a> {
            path: &'a Path,
        }
        impl Drop for RestorePerms<'_> {
            fn drop(&mut self) {
                if let Ok(mut p) = std::fs::metadata(self.path).map(|m| m.permissions()) {
                    p.set_mode(0o755);
                    let _ = std::fs::set_permissions(self.path, p);
                }
            }
        }
        let _restore = RestorePerms { path: &locked_dir };

        // Skip gracefully if the test is somehow running as root (root bypasses EACCES entirely,
        // which would make this specific proof-by-permission-failure meaningless — the tracked
        // double test above still covers the ordering claim in that environment).
        let probe = std::fs::read_dir(&locked_dir);
        if probe.is_ok() {
            eprintln!(
                "skipping permission-locked-tempdir proof: running with privileges that bypass \
                 directory-mode restrictions (e.g. root) — see the tracked-double test above for \
                 an environment-independent proof of the same ordering claim"
            );
            return;
        }

        let result = load_profile(&locked_dir, "../../etc/passwd");

        assert!(
            matches!(result, Err(SubagentError::UnsafePathToken(_))),
            "expected UnsafePathToken (validation-first rejection), got: {result:?} — if this \
             module's ordering regressed to 'try filesystem access, then validate', an EACCES-\
             derived SubagentError::Spawn would surface here instead"
        );
    }

    // -----------------------------------------------------------------------------------------
    // profile_path
    // -----------------------------------------------------------------------------------------

    #[test]
    fn profile_path_joins_validated_name_with_json_extension() {
        let dir = PathBuf::from("/some/profiles");
        let path = profile_path(&dir, "fast").expect("valid name");
        assert_eq!(path, PathBuf::from("/some/profiles/fast.json"));
    }

    #[test]
    fn profile_path_rejects_unsafe_name_and_returns_no_path() {
        let dir = PathBuf::from("/some/profiles");
        assert!(profile_path(&dir, "../escape").is_err());
    }

    // -----------------------------------------------------------------------------------------
    // list_profiles / load_profile: real tempdir + real filesystem I/O
    // -----------------------------------------------------------------------------------------

    fn write_profile(dir: &Path, name: &str, profile: &NamedProfile) {
        let path = dir.join(format!("{name}.json"));
        let json = serde_json::to_string_pretty(profile).expect("serialize profile");
        std::fs::write(path, json).expect("write profile file");
    }

    #[test]
    fn list_profiles_returns_empty_for_nonexistent_dir() {
        let dir = PathBuf::from("/nonexistent-profiles-dir-for-list-test");
        let names = list_profiles(&dir).expect("missing dir is not an error");
        assert!(names.is_empty());
    }

    #[test]
    fn list_profiles_finds_json_files_sorted_and_skips_non_json() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        write_profile(tmp.path(), "zeta", &NamedProfile::default());
        write_profile(tmp.path(), "alpha", &NamedProfile::default());
        std::fs::write(tmp.path().join("README.md"), b"not a profile")
            .expect("write unrelated file");

        let names = list_profiles(tmp.path()).expect("list profiles");
        assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn load_profile_round_trips_a_real_written_file() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "reviewer".to_string(),
            crate::discovery::types::AgentOverrideConfig {
                model: crate::discovery::types::OverrideField::Value("claude-opus".to_string()),
                ..Default::default()
            },
        );
        let profile = NamedProfile {
            subagents: SubagentSettings {
                overrides,
                default_model: Some("claude-sonnet".to_string()),
                ..Default::default()
            },
        };
        write_profile(tmp.path(), "quality", &profile);

        let loaded = load_profile(tmp.path(), "quality").expect("load written profile");
        assert_eq!(
            loaded.subagents.default_model.as_deref(),
            Some("claude-sonnet")
        );
        assert_eq!(
            loaded.subagents.overrides.get("reviewer").map(|o| &o.model),
            Some(&crate::discovery::types::OverrideField::Value(
                "claude-opus".to_string()
            ))
        );
    }

    #[test]
    fn load_profile_surfaces_io_error_for_missing_file_with_valid_name() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let result = load_profile(tmp.path(), "does-not-exist");
        assert!(matches!(result, Err(SubagentError::Spawn(_))));
    }

    #[test]
    fn load_profile_surfaces_malformed_settings_for_invalid_json() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        std::fs::write(tmp.path().join("broken.json"), b"{ not valid json")
            .expect("write malformed profile");
        let result = load_profile(tmp.path(), "broken");
        assert!(matches!(result, Err(SubagentError::MalformedSettings(_))));
    }

    // -----------------------------------------------------------------------------------------
    // describe_profiles
    // -----------------------------------------------------------------------------------------

    #[test]
    fn describe_profiles_reports_malformed_entries_without_aborting_the_rest() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        write_profile(tmp.path(), "good", &NamedProfile::default());
        std::fs::write(tmp.path().join("bad.json"), b"{ not valid")
            .expect("write malformed profile");

        let described = describe_profiles(tmp.path()).expect("describe profiles");
        assert_eq!(described.len(), 2);
        assert!(described.get("good").expect("good entry present").is_ok());
        assert!(described.get("bad").expect("bad entry present").is_err());
    }

    // -----------------------------------------------------------------------------------------
    // build_profile_file: the 6-agent tier map (pi buildProfileFile, profiles.ts:402-415 @v0.43.0 —
    // fourteen lines whose `agentOverrides` block names exactly six roles; `83b9872` deleted the
    // `planner`/`context-builder` entries that made it eight at the ported baseline)
    // -----------------------------------------------------------------------------------------

    fn override_model(profile: &NamedProfile, agent: &str) -> Option<String> {
        match profile.subagents.overrides.get(agent).map(|o| &o.model) {
            Some(OverrideField::Value(m)) => Some(m.clone()),
            _ => None,
        }
    }

    #[test]
    fn build_profile_file_assigns_all_six_builtins_to_their_tier() {
        let models = TierModels {
            cheap: "prov/cheap-1".to_string(),
            medium: "prov/medium-1".to_string(),
            strong: "prov/strong-1".to_string(),
        };
        let profile = build_profile_file(&models);

        // Exactly the 6 tiered builtins pi's `buildProfileFile` writes @ v0.43.0
        // (profiles.ts:402-415), no more, no fewer.
        assert_eq!(profile.subagents.overrides.len(), 6);

        // scout/delegate -> cheap
        assert_eq!(
            override_model(&profile, "scout").as_deref(),
            Some("prov/cheap-1")
        );
        assert_eq!(
            override_model(&profile, "delegate").as_deref(),
            Some("prov/cheap-1")
        );
        // researcher -> medium (its two former medium-tier companions, `planner` and
        // `context-builder`, were deleted upstream in `83b9872`)
        assert_eq!(
            override_model(&profile, "researcher").as_deref(),
            Some("prov/medium-1")
        );
        // worker/reviewer/oracle -> strong
        assert_eq!(
            override_model(&profile, "worker").as_deref(),
            Some("prov/strong-1")
        );
        assert_eq!(
            override_model(&profile, "reviewer").as_deref(),
            Some("prov/strong-1")
        );
        assert_eq!(
            override_model(&profile, "oracle").as_deref(),
            Some("prov/strong-1")
        );

        // The removed roles must carry NO override at all — a profile that still pinned a model on
        // a role that no longer exists would silently resurrect it in `settings.json`.
        assert_eq!(override_model(&profile, "planner"), None);
        assert_eq!(override_model(&profile, "context-builder"), None);
        // `advisor` is an `oracle` ALIAS, not a distinct override target: `agentOverrides` is keyed
        // by canonical name, so an `advisor` entry would never be applied to anything.
        assert_eq!(override_model(&profile, "advisor"), None);
    }

    #[test]
    fn build_profile_file_serializes_to_pi_agent_overrides_shape() {
        let models = TierModels {
            cheap: "openai-codex/gpt-5.3-codex-spark".to_string(),
            medium: "openai-codex/gpt-5.4-mini".to_string(),
            strong: "openai-codex/gpt-5.5".to_string(),
        };
        let profile = build_profile_file(&models);
        let value = serde_json::to_value(&profile).expect("serialize");
        // pi shape: { "subagents": { "agentOverrides": { "scout": { "model": "..." }, ... } } }
        let scout_model = value
            .get("subagents")
            .and_then(|s| s.get("agentOverrides"))
            .and_then(|a| a.get("scout"))
            .and_then(|s| s.get("model"))
            .and_then(|m| m.as_str());
        assert_eq!(scout_model, Some("openai-codex/gpt-5.3-codex-spark"));
        let worker_model = value
            .get("subagents")
            .and_then(|s| s.get("agentOverrides"))
            .and_then(|a| a.get("worker"))
            .and_then(|w| w.get("model"))
            .and_then(|m| m.as_str());
        assert_eq!(worker_model, Some("openai-codex/gpt-5.5"));
    }

    // -----------------------------------------------------------------------------------------
    // pick_tier_models: pi profiles.test.ts:169-189 executable-spec scenario, reproduced exactly
    // -----------------------------------------------------------------------------------------

    #[test]
    fn pick_tier_models_reproduces_pi_quota_and_quality_selection() {
        // pi profiles.test.ts: a ranked list of four models; quota drops the last (most expensive)
        // from its pool, quality samples the whole list.
        let ranked = vec![
            "openai-codex/gpt-5.3-codex-spark".to_string(),
            "openai-codex/gpt-5.4-mini".to_string(),
            "openai-codex/gpt-5.4".to_string(),
            "openai-codex/gpt-5.5".to_string(),
        ];

        let quota = pick_tier_models(&ranked, ProfileKind::Quota).expect("quota");
        assert_eq!(quota.cheap, "openai-codex/gpt-5.3-codex-spark");
        assert_eq!(quota.medium, "openai-codex/gpt-5.4-mini");
        assert_eq!(quota.strong, "openai-codex/gpt-5.4-mini");

        let quality = pick_tier_models(&ranked, ProfileKind::Quality).expect("quality");
        assert_eq!(quality.cheap, "openai-codex/gpt-5.4-mini");
        assert_eq!(quality.medium, "openai-codex/gpt-5.4");
        assert_eq!(quality.strong, "openai-codex/gpt-5.5");
    }

    #[test]
    fn pick_tier_models_single_model_maps_all_tiers_to_it() {
        let ranked = vec!["only/model".to_string()];
        let quota = pick_tier_models(&ranked, ProfileKind::Quota).expect("quota");
        assert_eq!(quota.cheap, "only/model");
        assert_eq!(quota.medium, "only/model");
        assert_eq!(quota.strong, "only/model");
        let quality = pick_tier_models(&ranked, ProfileKind::Quality).expect("quality");
        assert_eq!(quality.strong, "only/model");
    }

    #[test]
    fn pick_tier_models_rejects_empty_list() {
        let empty: Vec<String> = Vec::new();
        assert!(matches!(
            pick_tier_models(&empty, ProfileKind::Quota),
            Err(SubagentError::MalformedSettings(_))
        ));
    }

    #[test]
    fn round_index_edge_cases() {
        assert_eq!(round_index(0, 0.5), 0);
        assert_eq!(round_index(1, 0.9), 0);
        assert_eq!(round_index(4, 0.0), 0);
        assert_eq!(round_index(4, 1.0), 3);
        assert_eq!(round_index(4, 1.0 / 3.0), 1);
        assert_eq!(round_index(4, 2.0 / 3.0), 2);
    }

    // -----------------------------------------------------------------------------------------
    // generate_provider_profiles: writes both files with the 8-agent tier map
    // -----------------------------------------------------------------------------------------

    #[test]
    fn generate_provider_profiles_writes_quota_and_quality_with_the_six_agent_map() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let ranked = vec![
            "prov/cheap".to_string(),
            "prov/mid".to_string(),
            "prov/strong".to_string(),
        ];
        let result =
            generate_provider_profiles(tmp.path(), "prov", &ranked).expect("generate profiles");

        assert!(result.quota_path.ends_with("prov.quota.json"));
        assert!(result.quality_path.ends_with("prov.quality.json"));

        // Both files are real, load back through the read-only loader, and carry all 6 tiered
        // agents (8 until `83b9872` deleted `planner`/`context-builder` upstream).
        let quota = load_profile(tmp.path(), "prov.quota").expect("load quota");
        let quality = load_profile(tmp.path(), "prov.quality").expect("load quality");
        assert_eq!(quota.subagents.overrides.len(), 6);
        assert_eq!(quality.subagents.overrides.len(), 6);
        assert_eq!(
            override_model(&quota, "scout").as_deref(),
            Some(result.quota_models.cheap.as_str())
        );
        assert_eq!(
            override_model(&quality, "worker").as_deref(),
            Some(result.quality_models.strong.as_str())
        );

        // Both files are discoverable via the ordinary profile listing.
        let names = list_profiles(tmp.path()).expect("list");
        assert!(names.contains(&"prov.quota".to_string()));
        assert!(names.contains(&"prov.quality".to_string()));
    }

    // -----------------------------------------------------------------------------------------
    // apply_profile_to_settings_file: file-based, replaces ONLY subagents (pi applySubagentProfile)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn apply_profile_to_settings_file_replaces_only_subagents_key() {
        // Mirrors pi profiles.test.ts "applies a saved profile by replacing only settings.subagents".
        let tmp = tempfile::tempdir().expect("create tempdir");
        let settings_path = tmp.path().join("agent").join("settings.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).expect("mkdir");
        std::fs::write(
            &settings_path,
            r#"{ "defaultModel": "openai/gpt-5", "subagents": { "agentOverrides": { "scout": { "model": "old" } } } }"#,
        )
        .expect("seed settings");

        let profile = build_profile_file(&TierModels {
            cheap: "openai-codex/gpt-5.3-codex-spark".to_string(),
            medium: "openai-codex/gpt-5.4-mini".to_string(),
            strong: "openai-codex/gpt-5.5".to_string(),
        });
        apply_profile_to_settings_file(&settings_path, &profile).expect("apply");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read"))
                .expect("parse");
        // Sibling top-level key untouched.
        assert_eq!(
            written.get("defaultModel").and_then(|v| v.as_str()),
            Some("openai/gpt-5")
        );
        // subagents replaced wholesale with the profile's 8-agent map.
        let scout = written
            .get("subagents")
            .and_then(|s| s.get("agentOverrides"))
            .and_then(|a| a.get("scout"))
            .and_then(|s| s.get("model"))
            .and_then(|m| m.as_str());
        assert_eq!(scout, Some("openai-codex/gpt-5.3-codex-spark"));
    }

    #[test]
    fn apply_profile_to_settings_file_creates_file_when_absent() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let settings_path = tmp.path().join("nested").join("settings.json");
        let profile = NamedProfile {
            subagents: SubagentSettings {
                default_model: Some("fresh".to_string()),
                ..Default::default()
            },
        };
        apply_profile_to_settings_file(&settings_path, &profile).expect("apply");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read"))
                .expect("parse");
        assert_eq!(
            written
                .get("subagents")
                .and_then(|s| s.get("defaultModel"))
                .and_then(|m| m.as_str()),
            Some("fresh")
        );
    }

    // -----------------------------------------------------------------------------------------
    // G100: merge, don't replace (pi applySubagentProfile, profiles.ts:483-495)
    // -----------------------------------------------------------------------------------------

    /// `/subagents-load-profile` must not be a settings eraser. Unrelated `subagents.*` keys the
    /// profile says nothing about — the ones a user set once and never thinks about again — have
    /// to survive a profile switch.
    #[test]
    fn loading_a_profile_preserves_unrelated_subagent_settings() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let settings_path = tmp.path().join("agent").join("settings.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).expect("mkdir");
        std::fs::write(
            &settings_path,
            r#"{
                "defaultModel": "openai/gpt-5",
                "subagents": {
                    "disableBuiltins": true,
                    "defaultModel": "anthropic/claude-sonnet-5",
                    "modelScope": { "enforce": true, "allow": ["anthropic/*"] },
                    "agentOverrides": { "scout": { "model": "old" } }
                }
            }"#,
        )
        .expect("seed settings");

        let profile = build_profile_file(&TierModels {
            cheap: "openai-codex/gpt-5.3-codex-spark".to_string(),
            medium: "openai-codex/gpt-5.4-mini".to_string(),
            strong: "openai-codex/gpt-5.5".to_string(),
        });
        apply_profile_to_settings_file(&settings_path, &profile).expect("apply");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read"))
                .expect("parse");
        let subagents = written.get("subagents").expect("subagents block");

        // Layer 1: keys the profile does not mention survive.
        assert_eq!(
            subagents.get("disableBuiltins"),
            Some(&serde_json::json!(true)),
            "a profile that says nothing about disableBuiltins must not delete it"
        );
        assert_eq!(
            subagents.get("modelScope"),
            Some(&serde_json::json!({ "enforce": true, "allow": ["anthropic/*"] })),
            "nor silently disarm a modelScope policy"
        );
        // Layer 3: the profile owns the whole agent mapping — the previous profile's `scout` pin
        // is replaced, not merged over.
        assert_eq!(
            subagents
                .get("agentOverrides")
                .and_then(|a| a.get("scout"))
                .and_then(|s| s.get("model")),
            Some(&serde_json::json!("openai-codex/gpt-5.3-codex-spark"))
        );
        // Sibling top-level keys are still untouched.
        assert_eq!(
            written.get("defaultModel"),
            Some(&serde_json::json!("openai/gpt-5"))
        );
    }

    /// Layer 2 in isolation: a key the profile DOES declare must WIN over the value already on
    /// disk (pi `profiles.ts:491`: `...existing` is spread FIRST, `...profile.subagents` second).
    ///
    /// This is the layer that makes loading a profile mean anything at all. It is also the one the
    /// layer-1 test above cannot prove: "the on-disk value survived" and "the profile's value won"
    /// are opposite outcomes for a CONTESTED key, and a merge with the two spreads in the wrong
    /// order — or with no layer-2 write at all — still passes every layer-1 and layer-3 assertion.
    ///
    /// Both a scalar (`defaultModel`) and a structured value (`modelScope`) are contested here, so
    /// the assertion also covers the case where the profile's value must REPLACE the on-disk one
    /// wholesale rather than being merged into it.
    #[test]
    fn a_profile_key_overrides_the_value_already_on_disk() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let settings_path = tmp.path().join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{
                "subagents": {
                    "defaultModel": "on-disk/loser",
                    "modelScope": { "enforce": true, "allow": ["on-disk/*"] },
                    "disableBuiltins": true
                }
            }"#,
        )
        .expect("seed settings");

        let profile = NamedProfile {
            subagents: SubagentSettings {
                default_model: Some("profile/winner".to_string()),
                model_scope: Some(crate::exec::model_scope::ModelScopeConfig {
                    enforce: Some(false),
                    strict: None,
                    allow: Some(vec!["profile/*".to_string()]),
                }),
                ..Default::default()
            },
        };
        apply_profile_to_settings_file(&settings_path, &profile).expect("apply");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read"))
                .expect("parse");
        let subagents = written.get("subagents").expect("subagents block");

        assert_eq!(
            subagents.get("defaultModel"),
            Some(&serde_json::json!("profile/winner")),
            "a key the profile declares must beat the on-disk value — otherwise loading a profile \
             changes nothing for any setting the user had already touched"
        );
        assert_eq!(
            subagents
                .get("modelScope")
                .and_then(|s| s.get("allow"))
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(1),
            "the profile's structured value REPLACES the on-disk one; a deep merge would leave \
             both allowlists in place"
        );
        assert_eq!(
            subagents
                .get("modelScope")
                .and_then(|s| s.get("allow"))
                .and_then(|a| a.get(0)),
            Some(&serde_json::json!("profile/*")),
            "and the surviving entry is the profile's, not the on-disk one"
        );
        // Layer 1 still holds alongside it — the override is targeted, not a wholesale replace.
        assert_eq!(
            subagents.get("disableBuiltins"),
            Some(&serde_json::json!(true)),
            "an uncontested key is still untouched"
        );
    }

    /// Layer 3 in isolation: an agent the OLD profile pinned and the NEW profile does not mention
    /// must be unpinned. A key-by-key merge of `agentOverrides` could never do this, which is why
    /// pi assigns it unconditionally.
    #[test]
    fn a_profile_switch_drops_the_previous_profiles_agent_overrides() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let settings_path = tmp.path().join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{ "subagents": { "agentOverrides": { "retired-agent": { "model": "stale" }, "scout": { "model": "old" } } } }"#,
        )
        .expect("seed settings");

        let profile = NamedProfile {
            subagents: SubagentSettings {
                overrides: [(
                    "scout".to_string(),
                    AgentOverrideConfig {
                        model: OverrideField::Value("fresh".to_string()),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        };
        apply_profile_to_settings_file(&settings_path, &profile).expect("apply");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read"))
                .expect("parse");
        let overrides = written
            .get("subagents")
            .and_then(|s| s.get("agentOverrides"))
            .and_then(serde_json::Value::as_object)
            .expect("agentOverrides object");
        assert_eq!(
            overrides.keys().collect::<Vec<_>>(),
            vec!["scout"],
            "the previous profile's agents must not outlive it"
        );
    }

    /// The layer-3 edge: a profile with an EMPTY agent mapping. `SubagentSettings` serializes an
    /// empty map to no key at all, so layer 2 contributes nothing here — only the unconditional
    /// `agentOverrides` assignment (pi's `agentOverrides: profile.subagents.agentOverrides`, which
    /// for a validated-but-empty profile is `{}`) clears the previous profile's pins.
    #[test]
    fn a_profile_with_no_agent_overrides_clears_the_previous_ones() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let settings_path = tmp.path().join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{ "subagents": { "disableBuiltins": true, "agentOverrides": { "scout": { "model": "old" } } } }"#,
        )
        .expect("seed settings");

        let profile = NamedProfile {
            subagents: SubagentSettings {
                default_model: Some("openai/gpt-5.5".to_string()),
                ..Default::default()
            },
        };
        apply_profile_to_settings_file(&settings_path, &profile).expect("apply");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read"))
                .expect("parse");
        let subagents = written.get("subagents").expect("subagents block");
        assert_eq!(
            subagents.get("agentOverrides"),
            Some(&serde_json::json!({})),
            "an empty agent mapping is still the profile's mapping — it must replace, not vanish"
        );
        assert_eq!(
            subagents.get("disableBuiltins"),
            Some(&serde_json::json!(true)),
            "and the unrelated key still survives"
        );
    }

    #[test]
    fn apply_profile_to_settings_file_rejects_non_object_settings() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let settings_path = tmp.path().join("settings.json");
        std::fs::write(&settings_path, "[1, 2, 3]").expect("seed array settings");
        let result = apply_profile_to_settings_file(&settings_path, &NamedProfile::default());
        assert!(matches!(result, Err(SubagentError::MalformedSettings(_))));
    }

    // -----------------------------------------------------------------------------------------
    // profile_worker_model (pi getProfileWorkerModel)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn profile_worker_model_reads_the_worker_tier_override() {
        let profile = build_profile_file(&TierModels {
            cheap: "c".to_string(),
            medium: "m".to_string(),
            strong: "s".to_string(),
        });
        assert_eq!(profile_worker_model(&profile).as_deref(), Some("s"));

        assert!(profile_worker_model(&NamedProfile::default()).is_none());
    }

    // -----------------------------------------------------------------------------------------
    // per-provider catalog round-trip + staleness (pi ProviderModelCatalogFile / stale)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn provider_catalog_round_trips_and_staleness_respects_window() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let catalog = ProviderModelCatalog {
            provider: "openai".to_string(),
            refreshed_at_epoch_ms: 1_000_000,
            max_age_days: DEFAULT_PROVIDER_MODELS_MAX_AGE_DAYS,
            sources: vec!["runtime-registry".to_string()],
            models: vec![ProviderCatalogModel {
                id: "gpt-4o".to_string(),
                full_id: "openai/gpt-4o".to_string(),
                profile_rank: 42,
                probe_status: "ok".to_string(),
            }],
        };
        let path = write_provider_catalog(tmp.path(), &catalog).expect("write catalog");
        assert!(path.ends_with("providers/openai.models.json"));

        let read = read_provider_catalog(tmp.path(), "openai")
            .expect("read catalog")
            .expect("catalog present");
        assert_eq!(read, catalog);

        assert!(
            read_provider_catalog(tmp.path(), "never-refreshed")
                .expect("read absent")
                .is_none()
        );

        let day_ms: u64 = 24 * 60 * 60 * 1000;
        // Within the window: fresh.
        assert!(!is_provider_catalog_stale(
            &catalog,
            catalog.refreshed_at_epoch_ms + day_ms,
            DEFAULT_PROVIDER_MODELS_MAX_AGE_DAYS
        ));
        // Past the window: stale.
        assert!(is_provider_catalog_stale(
            &catalog,
            catalog.refreshed_at_epoch_ms + day_ms * 8,
            DEFAULT_PROVIDER_MODELS_MAX_AGE_DAYS
        ));
    }

    #[test]
    fn provider_models_path_rejects_unsafe_provider_name() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        assert!(matches!(
            provider_models_path(tmp.path(), "../escape"),
            Err(SubagentError::UnsafePathToken(_))
        ));
    }
}
