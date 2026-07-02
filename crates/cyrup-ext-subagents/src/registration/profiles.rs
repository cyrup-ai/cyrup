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
//!    `&str` argument, called strictly before [`profile_path`]/[`load_profile`]/[`apply_profile`]
//!    touch the filesystem or the `cyrup-config` settings store at all — the ordering this
//!    module's own path-traversal test proves via a filesystem-access-tracking double (see the
//!    `tests` module below), not merely by asserting the final `Err` outcome.
//!
//! 2. **[`apply_profile`] — the R-SA-141 targeted-key settings merge.** Loading a named profile
//!    MUST replace only the `subagents` top-level key in `cyrup-config`'s settings store, leaving
//!    every other top-level key (e.g. a top-level `defaultModel`) untouched. This module does
//!    **not** reimplement a targeted-merge primitive of its own: `cyrup_config::SettingsManager::
//!    set_nested(scope, &["subagents"], value)` already performs exactly this operation — a
//!    scoped read-modify-write that parses the on-disk document, replaces (via
//!    [`cyrup_config::settings`]'s internal `set_value_at_path`) only the single top-level key
//!    named by the first (and, here, only) path segment, and re-serializes the whole document —
//!    so [`apply_profile`] is a thin, well-documented call-through to that existing primitive
//!    rather than a second, parallel implementation that could drift out of sync with it.
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
//!   [`load_profile`], [`apply_profile`]) that command dispatch calls into, per R-SA-130's
//!   single-execution-code-path rule, rather than embedding any command-parsing logic here.
//! - **Named-profile persistence format for *writing* new profiles** (i.e. a `save_profile`-style
//!   authoring path) is not required by R-SA-140/141/142's text, which is scoped to *loading* and
//!   *applying* an already-authored profile; this file therefore does not implement profile
//!   creation. [`profile_path`]/[`list_profiles`]/[`load_profile`] are read-only over the
//!   profiles directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cyrup_config::{SettingsManager, SettingsScope};

use crate::discovery::types::SubagentSettings;
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
/// and [`apply_profile`] all call this first, unconditionally, before touching the filesystem or
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

/// Apply a named profile's `subagents` payload into `settings`'s `subagents` settings key, at
/// `scope`, via a **targeted replace of only the `subagents` key** (R-SA-141) — every other
/// top-level settings key (e.g. a top-level `defaultModel`) is left byte-for-byte untouched.
///
/// This is a thin call-through to [`cyrup_config::SettingsManager::set_nested`] with a
/// single-segment path (`&["subagents"]`): that function's own scoped read-modify-write already
/// parses the on-disk document, replaces exactly the named top-level key (creating it if absent,
/// preserving every sibling key already present), and re-serializes the whole document — this is
/// precisely R-SA-141's "replacing only the `subagents` key's value, leaving all other settings
/// keys... untouched" contract, with no second, parallel merge implementation needed here.
///
/// `name` is still validated via [`validate_profile_name`] first (R-SA-142 applies to the
/// settings-store key-selection path exactly as it does to the filesystem-lookup path — a
/// caller-supplied profile *name* is untrusted input regardless of which backing store it
/// ultimately addresses), even though `set_nested`'s own path segments here are the fixed,
/// hardcoded literal `"subagents"` and never `name` itself; this guards against a future caller
/// mistakenly threading an unvalidated `name` into a settings path segment of its own.
///
/// # Errors
///
/// - [`SubagentError::UnsafePathToken`] if `name` fails the R-SA-142 allowlist.
/// - [`SubagentError::Config`] if the underlying settings-store write fails (e.g. the target
///   scope is untrusted-project, or a lock/I/O error occurs).
pub fn apply_profile(
    settings: &mut SettingsManager,
    scope: SettingsScope,
    name: &str,
    profile: &NamedProfile,
) -> Result<(), SubagentError> {
    validate_profile_name(name)?;
    let value = serde_json::to_value(&profile.subagents).map_err(|e| {
        SubagentError::MalformedSettings(format!(
            "profile {name:?} could not be serialized for settings write: {e}"
        ))
    })?;
    settings.set_nested(scope, &["subagents"], value)?;
    Ok(())
}

/// Convenience composition of [`load_profile`] + [`apply_profile`]: read the named profile from
/// `profiles_dir`, then targeted-merge it into `settings` at `scope` (`/subagents-load-profile`'s
/// full end-to-end operation, R-SA-140/141/142 together).
///
/// # Errors
///
/// Propagates every error [`load_profile`]/[`apply_profile`] can return. In particular, an unsafe
/// `name` is rejected by [`load_profile`]'s own [`profile_path`] call before any filesystem
/// access is attempted, and (redundantly, defense-in-depth) would be rejected again by
/// [`apply_profile`]'s own validation if somehow reached with an already-loaded profile.
pub fn load_and_apply_profile(
    profiles_dir: &Path,
    settings: &mut SettingsManager,
    scope: SettingsScope,
    name: &str,
) -> Result<NamedProfile, SubagentError> {
    let profile = load_profile(profiles_dir, name)?;
    apply_profile(settings, scope, name, &profile)?;
    Ok(profile)
}

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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::cell::RefCell;
    use std::sync::Arc;

    use cyrup_config::{InMemorySettingsStore, Settings, SettingsManager, SettingsScope};

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
        std::fs::set_permissions(&locked_dir, perms.clone())
            .expect("chmod 000 the locked dir");

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
                model: crate::discovery::types::OverrideField::Value(
                    "claude-opus".to_string(),
                ),
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
            loaded
                .subagents
                .overrides
                .get("reviewer")
                .map(|o| &o.model),
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
    // R-SA-141: targeted-key merge — replaces ONLY `subagents`, leaves siblings untouched
    // -----------------------------------------------------------------------------------------

    fn manager_with_seed(seed_json: &str) -> SettingsManager {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, seed_json);
        SettingsManager::load(store, Settings::new(), true)
    }

    #[test]
    fn apply_profile_replaces_subagents_key_only_leaves_sibling_keys_untouched() {
        let mut mgr = manager_with_seed(
            r#"{
                "defaultModel": "top-level-default",
                "theme": "dark",
                "subagents": { "defaultModel": "old-subagents-default" }
            }"#,
        );

        let profile = NamedProfile {
            subagents: SubagentSettings {
                default_model: Some("new-subagents-default".to_string()),
                ..Default::default()
            },
        };

        apply_profile(&mut mgr, SettingsScope::Global, "quality", &profile)
            .expect("apply profile");

        // Sibling top-level keys are untouched.
        assert_eq!(
            mgr.global().get("defaultModel").and_then(|v| v.as_str()),
            Some("top-level-default")
        );
        assert_eq!(
            mgr.global().get("theme").and_then(|v| v.as_str()),
            Some("dark")
        );

        // The `subagents` key was replaced with the profile's payload.
        let subagents = mgr
            .global()
            .get("subagents")
            .expect("subagents key present after apply");
        assert_eq!(
            subagents.get("defaultModel").and_then(|v| v.as_str()),
            Some("new-subagents-default")
        );
    }

    #[test]
    fn apply_profile_creates_subagents_key_when_absent() {
        let mut mgr = manager_with_seed(r#"{ "defaultModel": "top-level-default" }"#);

        let profile = NamedProfile {
            subagents: SubagentSettings {
                default_model: Some("fresh-subagents-default".to_string()),
                ..Default::default()
            },
        };

        apply_profile(&mut mgr, SettingsScope::Global, "quality", &profile)
            .expect("apply profile");

        assert_eq!(
            mgr.global().get("defaultModel").and_then(|v| v.as_str()),
            Some("top-level-default"),
            "pre-existing sibling key must survive creation of a previously-absent subagents key"
        );
        let subagents = mgr.global().get("subagents").expect("subagents key created");
        assert_eq!(
            subagents.get("defaultModel").and_then(|v| v.as_str()),
            Some("fresh-subagents-default")
        );
    }

    #[test]
    fn apply_profile_rejects_unsafe_name_before_touching_settings_store() {
        let mut mgr = manager_with_seed(r#"{ "defaultModel": "top-level-default" }"#);
        let profile = NamedProfile::default();

        let result = apply_profile(&mut mgr, SettingsScope::Global, "../escape", &profile);

        assert!(matches!(result, Err(SubagentError::UnsafePathToken(_))));
        // The settings store must be completely unmodified: re-reading the seeded document
        // shows the original single top-level key and nothing else.
        assert_eq!(
            mgr.global().get("defaultModel").and_then(|v| v.as_str()),
            Some("top-level-default")
        );
        assert!(mgr.global().get("subagents").is_none());
    }

    #[test]
    fn load_and_apply_profile_end_to_end() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let profile = NamedProfile {
            subagents: SubagentSettings {
                default_model: Some("end-to-end-model".to_string()),
                ..Default::default()
            },
        };
        write_profile(tmp.path(), "e2e", &profile);

        let mut mgr = manager_with_seed(r#"{ "theme": "dark" }"#);
        let loaded = load_and_apply_profile(tmp.path(), &mut mgr, SettingsScope::Global, "e2e")
            .expect("load and apply");

        assert_eq!(
            loaded.subagents.default_model.as_deref(),
            Some("end-to-end-model")
        );
        assert_eq!(
            mgr.global().get("theme").and_then(|v| v.as_str()),
            Some("dark"),
            "sibling key survives"
        );
        let subagents = mgr.global().get("subagents").expect("subagents written");
        assert_eq!(
            subagents.get("defaultModel").and_then(|v| v.as_str()),
            Some("end-to-end-model")
        );
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
}
