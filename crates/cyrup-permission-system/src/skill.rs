//! The skill-read enforcement surface (port of the skill helpers in pi `index.ts` +
//! `skill-prompt-sanitizer.ts`'s enforcement-facing pieces). The SINGLE parse of the
//! `<available_skills>` block and the `resolveSkillPromptEntries` orchestration (which produces BOTH
//! the enforcement entries AND the hide-sanitized prompt) live in [`crate::sanitize::skills`]; this
//! module owns the resolved [`SkillPromptEntry`] type + the read-path matching the `tool_call` gate
//! uses: a `read` whose path lands on a tracked skill is matched
//! ([`find_skill_path_match`]/[`infer_skill_entry_from_read_path`], pi `index.ts:2230-2303`) so the
//! gate can bypass a `read`-tool deny for an allowed skill, ask for an ask skill, or block a deny
//! skill. [`resolved_entry`] (the resolved-entry builder, pi `createResolvedSkillEntry`) is shared
//! with [`crate::sanitize::skills`] so the single parse feeds both consumers.

use crate::common::{self, normalize_path_for_comparison};
use crate::types::PermissionState;

/// The `.cyrup/agent/skills` project skill root parts (cyrup analog of pi `SKILLS_DIR_PARTS =
/// [".pi","agent","skills"]`, `index.ts:142`).
const SKILLS_DIR_PARTS: [&str; 3] = [".cyrup", "agent", "skills"];

/// A resolved, enforcement-tracked skill entry (pi `SkillPromptEntry`, `skill-prompt-sanitizer.ts:
/// 20-27`). `name`/`state` drive the decision; the two normalized paths drive read-path matching.
#[derive(Debug, Clone)]
pub struct SkillPromptEntry {
    /// The skill name (pi `entry.name`).
    pub name: String,
    /// The resolved permission state for this skill (pi `entry.state`, from
    /// `permissionManager.checkPermission("skill", {name}, agentName)`).
    pub state: PermissionState,
    /// The skill's declared location normalized for comparison (pi `normalizedLocation`).
    pub normalized_location: String,
    /// The skill's location DIRECTORY normalized for comparison (pi `normalizedBaseDir`).
    pub normalized_base_dir: String,
}

/// node `path.dirname` (minimal): strip trailing slashes then everything after the last `/`; a
/// leading-slash-only parent collapses to `/`, no slash at all to `.`.
fn dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => trimmed.get(..i).unwrap_or(".").to_string(),
        None => ".".to_string(),
    }
}

/// Build a resolved entry (pi `createResolvedSkillEntry`, `skill-prompt-sanitizer.ts:146-159`). Shared
/// with [`crate::sanitize::skills`] so the single `<available_skills>` parse builds the enforcement
/// entries and the hide-sanitized prompt from one pass.
#[must_use]
pub(crate) fn resolved_entry(
    name: String,
    location: &str,
    state: PermissionState,
    cwd: &str,
) -> SkillPromptEntry {
    SkillPromptEntry {
        name,
        state,
        normalized_location: normalize_path_for_comparison(location, cwd),
        normalized_base_dir: normalize_path_for_comparison(&dirname(location), cwd),
    }
}

/// pi `findSkillPathMatch` (`skill-prompt-sanitizer.ts:280-303`): an exact `normalizedLocation`
/// hit, else the entry whose `normalizedBaseDir` CONTAINS the path with the LONGEST base dir.
#[must_use]
pub fn find_skill_path_match<'a>(
    normalized_path: &str,
    entries: &'a [SkillPromptEntry],
) -> Option<&'a SkillPromptEntry> {
    if normalized_path.is_empty() || entries.is_empty() {
        return None;
    }
    for entry in entries {
        if !entry.normalized_location.is_empty() && normalized_path == entry.normalized_location {
            return Some(entry);
        }
    }
    let mut best: Option<&SkillPromptEntry> = None;
    for entry in entries {
        if entry.normalized_base_dir.is_empty()
            || !common::is_path_within_directory(normalized_path, &entry.normalized_base_dir)
        {
            continue;
        }
        if best.is_none_or(|b| entry.normalized_base_dir.len() > b.normalized_base_dir.len()) {
            best = Some(entry);
        }
    }
    best
}

/// The first path segment of `normalized_read_path` UNDER `normalized_skills_root` (pi
/// `extractSkillNameUnderRoot`, `index.ts:604-612`), or `None` if not within the root.
fn extract_skill_name_under_root(
    normalized_read_path: &str,
    normalized_skills_root: &str,
) -> Option<String> {
    if !common::is_path_within_directory(normalized_read_path, normalized_skills_root) {
        return None;
    }
    let relative = normalized_read_path
        .strip_prefix(normalized_skills_root)
        .unwrap_or(normalized_read_path)
        .trim_start_matches(['/', '\\']);
    relative.split(['/', '\\']).find(|part| !part.is_empty()).map(str::to_string)
}

/// pi `inferSkillEntryFromReadPath` (`index.ts:614-647`): when a read path is NOT one of the parsed
/// skill entries but DOES live under a known skills root (`<agent_dir>/skills` or
/// `<cwd>/.cyrup/agent/skills`), synthesize an entry named after its first-segment skill dir so the
/// gate still enforces the skill policy on it. `agent_dir` is the cyrup analog of pi `PI_AGENT_DIR`.
#[must_use]
pub fn infer_skill_entry_from_read_path(
    read_path: &str,
    cwd: &str,
    agent_dir: &str,
    state: PermissionState,
) -> Option<SkillPromptEntry> {
    let normalized_read_path = normalize_path_for_comparison(read_path, cwd);
    if normalized_read_path.is_empty() {
        return None;
    }
    let agent_skills_root = common::join_paths(agent_dir, "skills");
    let project_skills_root = SKILLS_DIR_PARTS.iter().fold(cwd.to_string(), |acc, seg| {
        common::join_paths(&acc, seg)
    });
    for skill_root in [agent_skills_root, project_skills_root] {
        let normalized_skill_root = normalize_path_for_comparison(&skill_root, cwd);
        if let Some(skill_name) =
            extract_skill_name_under_root(&normalized_read_path, &normalized_skill_root)
        {
            return Some(SkillPromptEntry {
                name: skill_name,
                state,
                normalized_location: normalized_read_path.clone(),
                normalized_base_dir: normalize_path_for_comparison(&dirname(read_path), cwd),
            });
        }
    }
    None
}

/// pi `extractSkillNameFromInput` (`index.ts:243-257`): the skill name of a `/skill:<name> …` slash
/// command, else `None`.
#[must_use]
pub fn extract_skill_name_from_input(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let after = trimmed.strip_prefix("/skill:")?;
    if after.is_empty() {
        return None;
    }
    // pi slices up to the FIRST whitespace char (NOT the first token), then trims — so `/skill: x`
    // (space right after the colon) yields `""` → None, exactly like pi's `afterPrefix.slice(0, 0)`.
    let name = match after.find(char::is_whitespace) {
        Some(i) => after.get(..i).unwrap_or(""),
        None => after,
    }
    .trim();
    if name.is_empty() { None } else { Some(name.to_string()) }
}

/// pi `formatSkillPathAskPrompt` (`index.ts:594-597`).
#[must_use]
pub fn format_skill_path_ask_prompt(
    skill: &SkillPromptEntry,
    read_path: &str,
    agent_name: Option<&str>,
) -> String {
    let subject = match agent_name {
        Some(a) => format!("Agent '{a}'"),
        None => "Current agent".to_string(),
    };
    format!("{subject} requested access to skill '{}' via '{read_path}'. Allow this read?", skill.name)
}

/// pi `formatSkillPathDenyReason` (`index.ts:599-602`).
#[must_use]
pub fn format_skill_path_deny_reason(_skill: &SkillPromptEntry, agent_name: Option<&str>) -> String {
    let subject = match agent_name {
        Some(a) => format!("Agent '{a}'"),
        None => "Current agent".to_string(),
    };
    format!("{subject} is not permitted to access this skill.")
}

/// pi skill-read confirmation-unavailable reason (`index.ts:2278`).
#[must_use]
pub fn skill_ask_unavailable_reason() -> String {
    "Accessing this skill requires approval, but no interactive UI is available.".to_string()
}

/// pi skill-read user-denied reason (`index.ts:2294-2295`).
#[must_use]
pub fn format_skill_user_denied_reason(denial_reason: Option<&str>) -> String {
    let suffix = denial_reason.map(|r| format!(" Reason: {r}.")).unwrap_or_default();
    format!("User denied access to this skill.{suffix}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn find_match_prefers_exact_location() {
        let entries = vec![SkillPromptEntry {
            name: "deploy".into(),
            state: PermissionState::Allow,
            normalized_location: "/x/skills/deploy/SKILL.md".into(),
            normalized_base_dir: "/x/skills/deploy".into(),
        }];
        assert!(find_skill_path_match("/x/skills/deploy/SKILL.md", &entries).is_some());
        assert!(find_skill_path_match("/x/skills/deploy/ref.md", &entries).is_some());
        assert!(find_skill_path_match("/x/other/f", &entries).is_none());
    }

    #[test]
    fn extract_slash_skill_name() {
        assert_eq!(extract_skill_name_from_input("/skill:deploy do it"), Some("deploy".into()));
        assert_eq!(extract_skill_name_from_input("/skill:"), None);
        assert_eq!(extract_skill_name_from_input("hello"), None);
    }
}
