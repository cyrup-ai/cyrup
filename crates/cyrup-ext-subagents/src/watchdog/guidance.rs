//! SUBA-234 — the reviewer's standing `WATCHDOG.md` instructions, a port of
//! `pi-subagents/src/watchdog/guidance.ts` @ad11b7ab (added by `1bb41617`/#1815).
//!
//! *"Read fresh on every review; project file first, then user file."* Two optional files —
//! `<project>/.cyrup/WATCHDOG.md` (upstream `<project>/.pi/WATCHDOG.md`, pi `getProjectConfigDir`)
//! and `<agent dir>/WATCHDOG.md` (pi `getAgentDir`) — each trimmed, the non-empty ones joined by a
//! blank line, the whole capped at [`WATCHDOG_GUIDANCE_MAX_CHARS`] from the FRONT, so the project's
//! instructions survive the cap and the user's are what gets cut. `guidance.watchdogMd: false`
//! turns the read off. A missing, unreadable or empty file contributes nothing; there is no error
//! path, as upstream's `readOptional` swallows every one.
//!
//! The text reaches the reviewer through [`super::review::build_watchdog_system_prompt`]'s
//! `Standing instructions from WATCHDOG.md` block (`review.ts:237` @ad11b7ab), read by
//! [`super::review::MainWatchdogReview`] at review time (`review.ts:325`).

use std::path::Path;

/// `WATCHDOG_GUIDANCE_MAX_CHARS` (`guidance.ts:5`).
pub const WATCHDOG_GUIDANCE_MAX_CHARS: usize = 8_000;

/// The file name both layers read (`guidance.ts:18`).
pub const WATCHDOG_GUIDANCE_FILE_NAME: &str = "WATCHDOG.md";

/// `readOptional(filePath)` (`guidance.ts:7-13`) — trimmed contents, or empty on any failure.
fn read_optional(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// `loadWatchdogGuidance(cwd, enabled)` (`guidance.ts:16-20`), over the process's real agent dir.
#[must_use]
pub fn load_watchdog_guidance(cwd: &Path, enabled: bool) -> String {
    load_watchdog_guidance_from(cwd, &crate::paths::agent_dir(), enabled)
}

/// [`load_watchdog_guidance`] with the agent dir supplied, so a test reads its own tree instead of
/// the operator's home.
#[must_use]
pub fn load_watchdog_guidance_from(cwd: &Path, agent_dir: &Path, enabled: bool) -> String {
    if !enabled {
        return String::new();
    }
    let joined = [
        crate::paths::project_config_dir(cwd),
        agent_dir.to_path_buf(),
    ]
    .iter()
    .map(|dir| read_optional(&dir.join(WATCHDOG_GUIDANCE_FILE_NAME)))
    .filter(|section| !section.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    // `.slice(0, WATCHDOG_GUIDANCE_MAX_CHARS)` — the head, by character.
    joined.chars().take(WATCHDOG_GUIDANCE_MAX_CHARS).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tree() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent = tmp.path().join("agent");
        std::fs::create_dir_all(crate::paths::project_config_dir(&cwd)).unwrap();
        std::fs::create_dir_all(&agent).unwrap();
        (tmp, cwd, agent)
    }

    #[test]
    fn project_guidance_comes_first_then_the_users() {
        let (_tmp, cwd, agent) = tree();
        std::fs::write(
            crate::paths::project_config_dir(&cwd).join(WATCHDOG_GUIDANCE_FILE_NAME),
            "  Project rule.  \n",
        )
        .unwrap();
        std::fs::write(agent.join(WATCHDOG_GUIDANCE_FILE_NAME), "\nUser rule.\n").unwrap();
        assert_eq!(
            load_watchdog_guidance_from(&cwd, &agent, true),
            "Project rule.\n\nUser rule."
        );
    }

    #[test]
    fn absent_files_and_the_off_switch_both_yield_nothing() {
        let (_tmp, cwd, agent) = tree();
        assert_eq!(load_watchdog_guidance_from(&cwd, &agent, true), "");
        std::fs::write(agent.join(WATCHDOG_GUIDANCE_FILE_NAME), "User rule.").unwrap();
        assert_eq!(load_watchdog_guidance_from(&cwd, &agent, false), "");
        assert_eq!(
            load_watchdog_guidance_from(&cwd, &agent, true),
            "User rule.",
            "one file alone is read without a separator"
        );
    }

    #[test]
    fn the_cap_keeps_the_head() {
        let (_tmp, cwd, agent) = tree();
        std::fs::write(
            crate::paths::project_config_dir(&cwd).join(WATCHDOG_GUIDANCE_FILE_NAME),
            "P".repeat(WATCHDOG_GUIDANCE_MAX_CHARS),
        )
        .unwrap();
        std::fs::write(agent.join(WATCHDOG_GUIDANCE_FILE_NAME), "user tail").unwrap();
        let loaded = load_watchdog_guidance_from(&cwd, &agent, true);
        assert_eq!(loaded.chars().count(), WATCHDOG_GUIDANCE_MAX_CHARS);
        assert!(!loaded.contains("user tail"));
    }
}
