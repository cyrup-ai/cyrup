//! SUBA-134 — the child session's human-readable name: pi `src/shared/child-session-name.ts`
//! @v0.71.0 (`5ed2a4d4`, v0.57.0..v0.67.0).
//!
//! Derived once at launch from the agent name and its task (or workflow node label) and threaded
//! two ways, as upstream does: into the CHILD, which names its own session with it so the child's
//! session file is identifiable in `/resume` and session browsers
//! ([`crate::prompt_runtime`]'s `before_agent_start`), and into the result's `sessionName`, so a
//! host rendering the parent stream can label each child row.
//!
//! The name is display-only. When the intercom bridge is active the child claims its machine
//! routing target as its intercom session id (the `intercom:session-identity` request) and keeps
//! this readable name; with an intercom that never asks, the routing target stays the session name
//! because it routes by name.

use crate::workflows::preview_display_text;

/// The child-env variable carrying the derived name to the child — pi's
/// `childRuntimeConfig.sessionName` (`child-runtime-config.ts:68`, `child-launch.ts:251`
/// @v0.71.0); cyrup's child runtime is env-driven.
pub const CHILD_SESSION_NAME_ENV: &str = "CYRUP_SUBAGENT_SESSION_NAME";

/// pi `TASK_EXCERPT_MAX_CHARS` (`:22`).
const TASK_EXCERPT_MAX_CHARS: usize = 60;
/// pi `CHILD_SESSION_NAME_MAX_CHARS` (`:24`) — the whole name stays one host UI row.
pub const CHILD_SESSION_NAME_MAX_CHARS: usize = 80;

/// pi `PROMPT_REDACTED` (`shared/utils.ts:13`): never build a name from redacted text.
const PROMPT_REDACTED: &str = "[prompt redacted]";

/// pi `deriveChildSessionName({ agent, task, label })` (`:26-44`): `"<agent>: <excerpt>"`, where
/// the excerpt is the label when there is one, else the task, each previewed to
/// [`TASK_EXCERPT_MAX_CHARS`]; just the agent or just the excerpt when the other is empty; `None`
/// when both are.
#[must_use]
pub fn derive_child_session_name(
    agent: Option<&str>,
    task: Option<&str>,
    label: Option<&str>,
) -> Option<String> {
    let agent = agent.map_or("", str::trim);
    let raw_label = label.map_or("", str::trim);
    let raw_task = task.map_or("", str::trim);
    let excerpt_source = if !raw_label.is_empty() && raw_label != PROMPT_REDACTED {
        raw_label
    } else if !raw_task.is_empty() && raw_task != PROMPT_REDACTED {
        raw_task
    } else {
        ""
    };
    let excerpt = if excerpt_source.is_empty() {
        String::new()
    } else {
        preview_display_text(excerpt_source, TASK_EXCERPT_MAX_CHARS)
    };
    let base = match (agent.is_empty(), excerpt.is_empty()) {
        (false, false) => format!("{agent}: {excerpt}"),
        (false, true) => agent.to_string(),
        (true, _) => excerpt,
    };
    if base.is_empty() {
        return None;
    }
    Some(preview_display_text(&base, CHILD_SESSION_NAME_MAX_CHARS))
}

/// The name one child of `agent` doing `task` runs under: a launcher-assigned name already in the
/// child env (a labelled step's — pi's `step.sessionName ?? deriveChildSessionName(…)`,
/// `subagent-runner.ts:828`) wins, else the derivation from the agent and task.
#[must_use]
pub fn resolve_child_session_name(
    agent: &str,
    task: &str,
    child_env: &std::collections::HashMap<String, String>,
) -> Option<String> {
    child_env
        .get(CHILD_SESSION_NAME_ENV)
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .or_else(|| derive_child_session_name(Some(agent), Some(task), None))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_agent_colon_excerpt_with_upstreams_bounds() {
        assert_eq!(
            derive_child_session_name(Some("worker"), Some("fix auth refresh"), None).as_deref(),
            Some("worker: fix auth refresh")
        );
        assert_eq!(
            derive_child_session_name(Some(" worker "), Some("  "), None).as_deref(),
            Some("worker")
        );
        assert_eq!(
            derive_child_session_name(None, Some("just a task"), None).as_deref(),
            Some("just a task")
        );
        assert_eq!(derive_child_session_name(Some(""), Some(""), None), None);
        // The label wins over the task; a redacted source is skipped.
        assert_eq!(
            derive_child_session_name(Some("w"), Some("task"), Some("Lane A")).as_deref(),
            Some("w: Lane A")
        );
        assert_eq!(
            derive_child_session_name(Some("w"), Some("[prompt redacted]"), None).as_deref(),
            Some("w")
        );
        // The excerpt is previewed to 60 units (57 + "..."), the whole to 80.
        let long = "x".repeat(100);
        let named = derive_child_session_name(Some("worker"), Some(&long), None).unwrap();
        assert_eq!(named, format!("worker: {}...", "x".repeat(57)));
        let agent = "a".repeat(90);
        let named = derive_child_session_name(Some(&agent), Some("t"), None).unwrap();
        assert_eq!(named.len(), 80);
        assert!(named.ends_with("..."));
    }

    #[test]
    fn a_launcher_assigned_name_wins_over_the_derivation() {
        let mut env = std::collections::HashMap::new();
        assert_eq!(
            resolve_child_session_name("worker", "fix it", &env).as_deref(),
            Some("worker: fix it")
        );
        env.insert(CHILD_SESSION_NAME_ENV.to_string(), "Lane A".to_string());
        assert_eq!(
            resolve_child_session_name("worker", "fix it", &env).as_deref(),
            Some("Lane A")
        );
    }
}
