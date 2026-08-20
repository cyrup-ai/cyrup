//! `<agent_dir>/mcp-onboarding.json` — `onboarding-state.ts` (13a §27, MCP-380).
//!
//! Three booleans and a fingerprint, persisted so the shared-config hint and the `/mcp setup` flow
//! each happen once. Consumers are `commands.ts` (`/mcp`) and `mcp-setup-panel.ts`.
//!
//! # The two details that are easy to get wrong
//!
//! **The read is *normalising*, not a plain deserialise.** A missing file, a parse failure or a
//! non-object all yield a **copy** of [`OnboardingState::default`]; otherwise `version` is *forced*
//! to `1` regardless of what the file said, both booleans are `=== true` coercions (so `"yes"`,
//! `1` and `null` all read as `false`), `lastDiscoveryFingerprint` survives only when it is a
//! string, and unknown keys are dropped. A `#[derive(Deserialize)]` would reject where upstream
//! coerces, so the read is written by hand over a [`serde_json::Value`].
//!
//! **The write is atomic and pid-scoped.** `mkdirSync(recursive)`, write
//! `` `${JSON.stringify(state, null, 2)}\n` `` to `` `${path}.${process.pid}.tmp` ``, then
//! `renameSync` onto the real path. The pid in the temp name is what stops two concurrent processes
//! from colliding on the temp file — without it, one process's partial write lands under the
//! other's rename.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::{McpError, McpResult};

/// The persisted schema. `version` is always `1`; there is no migration path because a
/// wrong-versioned file is simply re-normalised on read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingState {
    /// Always `1`. Forced on read.
    pub version: u32,
    /// Whether the "your MCP config is shared with other tools" hint has been shown.
    pub shared_config_hint_shown: bool,
    /// Whether `/mcp setup` has been completed at least once.
    pub setup_completed: bool,
    /// The discovery fingerprint in force when a flag was last set — how the hint re-arms when the
    /// user's host-config landscape changes. Omitted from the file entirely when absent, matching
    /// upstream's optional key (never written as `null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_discovery_fingerprint: Option<String>,
}

impl Default for OnboardingState {
    /// Upstream `DEFAULT_STATE` — `{version: 1, sharedConfigHintShown: false, setupCompleted: false}`
    /// with no fingerprint key.
    fn default() -> Self {
        Self {
            version: 1,
            shared_config_hint_shown: false,
            setup_completed: false,
            last_discovery_fingerprint: None,
        }
    }
}

/// `loadOnboardingState()` — never fails. Every failure mode (absent, unreadable, unparseable,
/// non-object) is upstream's "return a copy of the default".
#[must_use]
pub fn load_onboarding_state(path: &Path) -> OnboardingState {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return OnboardingState::default();
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&raw) else {
        return OnboardingState::default();
    };
    OnboardingState {
        // Forced, not read: upstream writes `version: 1` back regardless of the stored value.
        version: 1,
        // `=== true`, so any non-boolean (or a missing key) is `false`.
        shared_config_hint_shown: map.get("sharedConfigHintShown") == Some(&Value::Bool(true)),
        setup_completed: map.get("setupCompleted") == Some(&Value::Bool(true)),
        // Kept only when it is a string; a number or `null` is dropped, not stringified.
        last_discovery_fingerprint: map
            .get("lastDiscoveryFingerprint")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// `saveOnboardingState(state)` — create the parent directory, write to a pid-scoped temp file,
/// rename onto the target. The trailing newline is upstream's and is preserved so a diff of the
/// file against a hand-edited copy stays clean.
pub fn save_onboarding_state(path: &Path, state: &OnboardingState) -> McpResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| McpError::Io { path: parent.to_path_buf(), source })?;
    }
    let body = serde_json::to_string_pretty(state)
        .map_err(|e| McpError::Config(format!("serialising onboarding state: {e}")))?;

    // `${path}.${process.pid}.tmp` — the pid is the collision guard, not decoration.
    let mut temp = path.as_os_str().to_os_string();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = std::path::PathBuf::from(temp);

    std::fs::write(&temp, format!("{body}\n"))
        .map_err(|source| McpError::Io { path: temp.clone(), source })?;
    std::fs::rename(&temp, path).map_err(|source| {
        // A failed rename leaves the temp file behind; upstream's `renameSync` throw does too, but
        // there is no reason to keep it.
        let _ = std::fs::remove_file(&temp);
        McpError::Io { path: path.to_path_buf(), source }
    })
}

/// `updateOnboardingState(updater)` — load, mutate, save, return the saved value.
pub fn update_onboarding_state(
    path: &Path,
    update: impl FnOnce(&mut OnboardingState),
) -> McpResult<OnboardingState> {
    let mut state = load_onboarding_state(path);
    update(&mut state);
    save_onboarding_state(path, &state)?;
    Ok(state)
}

/// `markSharedConfigHintShown(fingerprint?)`. The fingerprint carried forward is
/// `fingerprint ?? state.lastDiscoveryFingerprint`, and the key is omitted entirely when both are
/// absent — which is why this takes `Option<&str>` rather than defaulting to `""`.
pub fn mark_shared_config_hint_shown(
    path: &Path,
    fingerprint: Option<&str>,
) -> McpResult<OnboardingState> {
    update_onboarding_state(path, |state| {
        state.shared_config_hint_shown = true;
        if let Some(fp) = fingerprint {
            state.last_discovery_fingerprint = Some(fp.to_string());
        }
    })
}

/// `markSetupCompleted(fingerprint?)` — same carry-forward rule as
/// [`mark_shared_config_hint_shown`].
pub fn mark_setup_completed(path: &Path, fingerprint: Option<&str>) -> McpResult<OnboardingState> {
    update_onboarding_state(path, |state| {
        state.setup_completed = true;
        if let Some(fp) = fingerprint {
            state.last_discovery_fingerprint = Some(fp.to_string());
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_reads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-onboarding.json");
        assert_eq!(load_onboarding_state(&path), OnboardingState::default());
    }

    #[test]
    fn read_normalises_rather_than_rejecting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-onboarding.json");
        std::fs::write(
            &path,
            r#"{"version":7,"sharedConfigHintShown":"yes","setupCompleted":true,
                "lastDiscoveryFingerprint":42,"extra":1}"#,
        )
        .unwrap();
        let state = load_onboarding_state(&path);
        assert_eq!(state.version, 1, "version is forced, not read");
        assert!(!state.shared_config_hint_shown, "`=== true`, so a truthy string is false");
        assert!(state.setup_completed);
        assert_eq!(state.last_discovery_fingerprint, None, "a non-string fingerprint is dropped");
    }

    #[test]
    fn unparseable_file_reads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-onboarding.json");
        std::fs::write(&path, "{{{").unwrap();
        assert_eq!(load_onboarding_state(&path), OnboardingState::default());
    }

    #[test]
    fn save_is_atomic_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("mcp-onboarding.json");
        let saved = mark_setup_completed(&path, Some("fp-1")).unwrap();
        assert!(saved.setup_completed);
        assert_eq!(saved.last_discovery_fingerprint.as_deref(), Some("fp-1"));

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.ends_with('\n'), "upstream writes a trailing newline");
        assert_eq!(load_onboarding_state(&path), saved);

        // No temp file survives a successful write.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn absent_fingerprint_is_carried_forward_not_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-onboarding.json");
        mark_setup_completed(&path, Some("fp-1")).unwrap();
        let after = mark_shared_config_hint_shown(&path, None).unwrap();
        assert_eq!(after.last_discovery_fingerprint.as_deref(), Some("fp-1"));
        assert!(after.setup_completed && after.shared_config_hint_shown);
    }
}
