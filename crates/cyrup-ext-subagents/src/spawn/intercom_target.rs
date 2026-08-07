//! Deterministic subagent↔supervisor intercom addressing — a faithful port of pi's
//! `pi-subagents/src/intercom/intercom-bridge.ts:83-97`.
//!
//! # Why this lives in `cyrup-ext-subagents`, not `cyrup-intercom`
//!
//! `cyrup-intercom` depends on `cyrup-ext-subagents` (its `seams.rs`/`extension.rs` import this
//! crate's `tui::intercom` channels + `background::RunId`), so the dependency edge forbids the
//! reverse import. The PARENT-side target computation (a supervisor's own presence target + each
//! child's deterministic label) runs at the subagents SPAWN site (`exec::build_attempt_spawn_plan`
//! writes the child's identity env; `extension::control_resume` addresses a live child), which is
//! here. So these two pure string functions are duplicated here rather than reused from
//! `cyrup-intercom::identity`. They MUST stay byte-identical to
//! [`cyrup_intercom::identity::presence_name`] / that crate's `resolve_subagent_intercom_target`
//! expectation so the two independently-produced strings match at the broker — the
//! [`tests`] below pin the exact formulas.
//!
//! # The child-bridge identity env vars
//!
//! The six vars a spawned child reads (`cyrup-intercom::identity`) to activate its
//! `contact_supervisor` tool + its broker presence. Two of them ([`crate::spawn::nested_events::RUN_ID_ENV`]
//! / [`crate::spawn::nested_events::CHILD_INDEX_ENV`]) are SHARED with the nested-events overlay —
//! the spawn site sets each once, satisfying both subsystems — so they are re-exported from there
//! rather than redeclared. The remaining four are declared here (mirroring
//! `cyrup-intercom::identity`'s `CYRUP_SUBAGENT_*` constants).

pub use crate::spawn::nested_events::{CHILD_INDEX_ENV, RUN_ID_ENV};

/// `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET` (pi `PI_SUBAGENT_ORCHESTRATOR_TARGET`,
/// `pi-args.ts:15,204-205`): the supervisor's addressable presence target the child's
/// `contact_supervisor` relays to. Read by `cyrup-intercom::identity::ENV_ORCH_TARGET`.
pub const ENV_ORCHESTRATOR_TARGET: &str = "CYRUP_SUBAGENT_ORCHESTRATOR_TARGET";
/// `CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID` (pi `PI_SUBAGENT_ORCHESTRATOR_SESSION_ID`): the
/// supervisor's stable session id, preferred over the target when set. Read by
/// `cyrup-intercom::identity::ENV_ORCH_SESSION_ID`. The spawn site leaves it UNSET (pi's `pi-args.ts`
/// itself never sets it — the child resolves the supervisor by the presence NAME in
/// [`ENV_ORCHESTRATOR_TARGET`], which the broker resolves by name); the constant exists so a caller
/// that DOES have a broker-resolvable stable id can set it without redeclaring the string.
pub const ENV_ORCHESTRATOR_SESSION_ID: &str = "CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID";
/// `CYRUP_SUBAGENT_CHILD_AGENT` (pi `PI_SUBAGENT_CHILD_AGENT`, `pi-args.ts:17,210-211`): the child's
/// own persona name (one of the four vars required for the metadata gate to activate). Read by
/// `cyrup-intercom::identity::ENV_CHILD_AGENT`.
pub const ENV_CHILD_AGENT: &str = "CYRUP_SUBAGENT_CHILD_AGENT";
/// `CYRUP_SUBAGENT_INTERCOM_SESSION_NAME` (pi `PI_SUBAGENT_INTERCOM_SESSION_NAME`,
/// `pi-args.ts:201-202`): the child's OWN deterministic presence label
/// ([`resolve_subagent_intercom_target`]) — the addressable name the parent steers. Read by
/// `cyrup-intercom::identity::ENV_INTERCOM_SESSION_NAME`.
pub const ENV_INTERCOM_SESSION_NAME: &str = "CYRUP_SUBAGENT_INTERCOM_SESSION_NAME";

/// pi `sanitizeIntercomTargetPart` (`intercom-bridge.ts:90-92`): lowercase, collapse every run of
/// characters outside `[a-z0-9_-]` to a single `-`, strip leading/trailing `-`, and fall back to
/// `"agent"` when the result is empty.
#[must_use]
pub fn sanitize_intercom_target_part(value: &str) -> String {
    let lowered = value.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed.to_string()
    }
}

/// pi `resolveSubagentIntercomTarget` (`intercom-bridge.ts:94-97`): the deterministic broker
/// presence label a spawned child registers under (and the parent addresses to steer it) —
/// `subagent-<sanitize(agent)>-<sanitize(run_id)>-<index+1>`.
#[must_use]
pub fn resolve_subagent_intercom_target(run_id: &str, agent: &str, index: usize) -> String {
    resolve_subagent_intercom_target_opt(run_id, agent, Some(index))
}

/// The index-OPTIONAL form of [`resolve_subagent_intercom_target`], matching upstream's signature
/// exactly: `resolveSubagentIntercomTarget(runId, agent, index?: number)` renders
/// `stepSuffix = index !== undefined ? `-${index + 1}` : ""` (`intercom-bridge.ts:94-97`
/// @v0.34.0), so an index-less caller gets the bare `subagent-<agent>-<run>` label with NO trailing
/// step number.
///
/// Every SPAWN site in this crate knows its child's flat index and therefore uses the `usize` form
/// above. The one caller that genuinely may not is
/// `SubagentExecutor::foreground_control_notifier`, which mirrors `emitControlNotification`'s
/// `resolveSubagentIntercomTarget(event.runId, event.agent, event.index)`
/// (`subagent-executor.ts:512-513` @v0.34.0) — `ControlEvent::index` is optional there, and
/// synthesising a `-1` suffix for an index-less event would both mis-address the child and change
/// the notice's dedup key.
#[must_use]
pub fn resolve_subagent_intercom_target_opt(
    run_id: &str,
    agent: &str,
    index: Option<usize>,
) -> String {
    let step_suffix = match index {
        Some(index) => format!("-{}", index + 1),
        None => String::new(),
    };
    format!(
        "subagent-{}-{}{step_suffix}",
        sanitize_intercom_target_part(agent),
        sanitize_intercom_target_part(run_id),
    )
}

/// pi `resolveIntercomSessionTarget` (`intercom-bridge.ts:83-88`): a supervisor's own addressable
/// presence target — the trimmed session name if present, else the unnamed-session alias
/// `subagent-chat-<id[0:8]>` (with a leading `session-` prefix stripped first). Byte-identical to
/// [`cyrup_intercom::identity::presence_name`] so the child's `orchestrator_target` (produced here,
/// at the spawn site) matches the string the supervisor's own intercom extension registers under.
#[must_use]
pub fn orchestrator_presence_target(session_name: Option<&str>, session_id: &str) -> String {
    if let Some(name) = session_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let normalized = session_id.strip_prefix("session-").unwrap_or(session_id);
    let short: String = normalized.chars().take(8).collect();
    format!("subagent-chat-{short}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn env_var_names_match_the_intercom_identity_contract() {
        // These strings are the cross-crate contract the child's `cyrup-intercom::identity` gate
        // reads. Pinned here (the two crates cannot import each other's constants) so a rename on
        // either side is caught by a failing test rather than a silently dead bridge.
        assert_eq!(ENV_ORCHESTRATOR_TARGET, "CYRUP_SUBAGENT_ORCHESTRATOR_TARGET");
        assert_eq!(ENV_ORCHESTRATOR_SESSION_ID, "CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID");
        assert_eq!(ENV_CHILD_AGENT, "CYRUP_SUBAGENT_CHILD_AGENT");
        assert_eq!(ENV_INTERCOM_SESSION_NAME, "CYRUP_SUBAGENT_INTERCOM_SESSION_NAME");
        assert_eq!(RUN_ID_ENV, "CYRUP_SUBAGENT_RUN_ID");
        assert_eq!(CHILD_INDEX_ENV, "CYRUP_SUBAGENT_CHILD_INDEX");
    }

    #[test]
    fn sanitize_matches_pi_regex_behavior() {
        // trim + lowercase + non-[a-z0-9_-] runs -> single '-' + strip edge '-'.
        assert_eq!(sanitize_intercom_target_part("  Research Bot  "), "research-bot");
        assert_eq!(sanitize_intercom_target_part("a//b__c--d"), "a-b__c--d");
        assert_eq!(sanitize_intercom_target_part("***"), "agent");
        assert_eq!(sanitize_intercom_target_part(""), "agent");
        assert_eq!(sanitize_intercom_target_part("MixédÇase"), "mix-d-ase");
    }

    #[test]
    fn resolve_target_is_agent_runid_index_plus_one() {
        assert_eq!(
            resolve_subagent_intercom_target("run-ABC", "Reviewer", 0),
            "subagent-reviewer-run-abc-1"
        );
        assert_eq!(
            resolve_subagent_intercom_target("run-ABC", "Reviewer", 2),
            "subagent-reviewer-run-abc-3"
        );
    }

    #[test]
    fn orchestrator_presence_target_matches_presence_name() {
        assert_eq!(orchestrator_presence_target(Some("  Alice "), "id"), "Alice");
        assert_eq!(
            orchestrator_presence_target(None, "session-deadbeefcafef00d"),
            "subagent-chat-deadbeef"
        );
        assert_eq!(orchestrator_presence_target(Some("   "), "abcdefghij"), "subagent-chat-abcdefgh");
    }
}
