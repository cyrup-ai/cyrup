//! Env-var identity adoption + addressing — a port of `pi-intercom/index.ts:20-119,368-386,856-888`.
//!
//! Env names mirror pi with the `CYRUP_` prefix (the memory rule: port the literal mechanism;
//! `cyrup-ext-subagents` already uses `CYRUP_SUBAGENT_*` mirroring `PI_SUBAGENT_*`). The
//! `contact_supervisor` tool is registered ONLY when [`read_child_orchestrator_metadata`] returns
//! `Some` (all of target/run-id/agent/child-index present — `index.ts:84-103,1162-1163`).

/// `DEFAULT_UNNAMED_SESSION_ALIAS_PREFIX = "subagent-chat"` (`index.ts:20`).
pub const DEFAULT_UNNAMED_SESSION_ALIAS_PREFIX: &str = "subagent-chat";

/// `CYRUP_INTERCOM_SESSION_ID` (`index.ts:441,606-615`, pi `PI_INTERCOM_SESSION_ID`).
///
/// Direction matters: upstream **publishes** this (`publishIntercomSessionId`, `index.ts:612-614`,
/// called from `startSessionRuntime`, `index.ts:946`) so a spawned CHILD inherits it and reads it
/// back as its SUPERVISOR's id — see [`read_child_orchestrator_metadata_from`]'s fallback below. A
/// session never re-reads it as its OWN registration id; that is
/// `ctx.sessionManager.getSessionId()` (cyrup: `HostServices::session_id()`, see
/// [`crate::connect::connect_once`]). Reading it for self-registration would make a child
/// re-register under its parent's id and take the parent's broker slot over.
pub const ENV_INTERCOM_SESSION_ID: &str = "CYRUP_INTERCOM_SESSION_ID";
/// `CYRUP_INTERCOM_ASK_TIMEOUT_MS` (`config.ts:8`, pi `PI_INTERCOM_ASK_TIMEOUT_MS`).
pub const ENV_INTERCOM_ASK_TIMEOUT_MS: &str = "CYRUP_INTERCOM_ASK_TIMEOUT_MS";
/// `CYRUP_INTERCOM_NAME_POLL_MS` (`index.ts:420-429`, pi `PI_INTERCOM_NAME_POLL_MS`).
pub const ENV_INTERCOM_NAME_POLL_MS: &str = "CYRUP_INTERCOM_NAME_POLL_MS";
/// `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET` (`index.ts:21`, pi `PI_SUBAGENT_ORCHESTRATOR_TARGET`).
pub const ENV_ORCH_TARGET: &str = "CYRUP_SUBAGENT_ORCHESTRATOR_TARGET";
/// `CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID` (`index.ts:22`) — the preferred, stable supervisor id.
pub const ENV_ORCH_SESSION_ID: &str = "CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID";
/// `CYRUP_SUBAGENT_RUN_ID` (`index.ts:25`).
pub const ENV_RUN_ID: &str = "CYRUP_SUBAGENT_RUN_ID";
/// `CYRUP_SUBAGENT_CHILD_AGENT` (`index.ts:26`).
pub const ENV_CHILD_AGENT: &str = "CYRUP_SUBAGENT_CHILD_AGENT";
/// `CYRUP_SUBAGENT_CHILD_INDEX` (`index.ts:27`).
pub const ENV_CHILD_INDEX: &str = "CYRUP_SUBAGENT_CHILD_INDEX";
/// `CYRUP_SUBAGENT_INTERCOM_SESSION_NAME` (`index.ts:28`) — the child's own presence label.
pub const ENV_INTERCOM_SESSION_NAME: &str = "CYRUP_SUBAGENT_INTERCOM_SESSION_NAME";

/// `ChildOrchestratorMetadata` (`index.ts:30-46`): the subagent-child ↔ supervisor addressing the
/// broker-routed ask/answer path is parameterized by. `index` is kept as a `String` (verbatim from
/// the env, used only in the formatted message body) exactly as pi keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildOrchestratorMetadata {
    /// The supervisor's addressable target (name or id) — `orchestrator_target`.
    pub orchestrator_target: String,
    /// The supervisor's stable session id, preferred for target resolution when present
    /// (`index.ts:880-888`).
    pub orchestrator_session_id: Option<String>,
    /// This run's id.
    pub run_id: String,
    /// This child's agent persona name.
    pub agent: String,
    /// This child's index within its fan-out group (verbatim string).
    pub index: String,
    /// This child's own presence label, if the launcher assigned one.
    pub session_name: Option<String>,
}

/// `readChildOrchestratorMetadata` (`index.ts:84-103`): resolve the child's supervisor addressing
/// from the process environment, or `None` when any of the four required vars
/// (target/run-id/agent/child-index) is missing/blank. `orchestrator_session_id` falls back from
/// [`ENV_ORCH_SESSION_ID`] to [`ENV_INTERCOM_SESSION_ID`] (`index.ts:86-87`).
#[must_use]
pub fn read_child_orchestrator_metadata() -> Option<ChildOrchestratorMetadata> {
    read_child_orchestrator_metadata_from(|k| std::env::var(k).ok())
}

/// The pure core of [`read_child_orchestrator_metadata`], parameterized over the env lookup so the
/// gate is unit-testable without touching process-global env state.
#[must_use]
pub fn read_child_orchestrator_metadata_from(
    env: impl Fn(&str) -> Option<String>,
) -> Option<ChildOrchestratorMetadata> {
    let trimmed = |k: &str| env(k).map(|v| v.trim().to_string()).filter(|s| !s.is_empty());

    let orchestrator_target = trimmed(ENV_ORCH_TARGET)?;
    let orchestrator_session_id =
        trimmed(ENV_ORCH_SESSION_ID).or_else(|| trimmed(ENV_INTERCOM_SESSION_ID));
    let run_id = trimmed(ENV_RUN_ID)?;
    let agent = trimmed(ENV_CHILD_AGENT)?;
    let index = trimmed(ENV_CHILD_INDEX)?;
    let session_name = trimmed(ENV_INTERCOM_SESSION_NAME);

    Some(ChildOrchestratorMetadata {
        orchestrator_target,
        orchestrator_session_id,
        run_id,
        agent,
        index,
        session_name,
    })
}

/// `resolveIntercomPresenceName` (`index.ts:379-386`): the trimmed session name if present, else the
/// unnamed-session alias `subagent-chat-<id[0:8]>` (with a leading `session-` prefix stripped first).
#[must_use]
pub fn presence_name(session_name: Option<&str>, session_id: &str) -> String {
    if let Some(name) = session_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let normalized = session_id.strip_prefix("session-").unwrap_or(session_id);
    let short: String = normalized.chars().take(8).collect();
    format!("{DEFAULT_UNNAMED_SESSION_ALIAS_PREFIX}-{short}")
}

/// `shortSessionId` (`index.ts:365-367`): the first 8 chars of a session id (for display).
#[must_use]
pub fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

/// The preferred supervisor target string to hand `IntercomClient::send` (`resolveSupervisorTarget`,
/// `index.ts:880-888`): prefer the stable `orchestrator_session_id`, else the `orchestrator_target`.
/// The broker's `findSessions` (`broker.ts:581-596`) then resolves id → name → unique prefix, so
/// passing the raw preferred target is sufficient without a client-side pre-resolution round trip.
#[must_use]
pub fn preferred_supervisor_target(meta: &ChildOrchestratorMetadata) -> String {
    meta.orchestrator_session_id
        .clone()
        .unwrap_or_else(|| meta.orchestrator_target.clone())
}

/// `formatChildOrchestratorMessage` (`index.ts:104-119`): the human-facing ask/update/interview body
/// the child sends to its supervisor over the broker.
#[must_use]
pub fn format_child_orchestrator_message(
    kind: ChildMessageKind,
    metadata: &ChildOrchestratorMetadata,
    message: &str,
) -> String {
    let heading = match kind {
        ChildMessageKind::Ask => "Subagent needs a supervisor decision.",
        ChildMessageKind::Interview => "Subagent requests a structured supervisor interview.",
        ChildMessageKind::Update => "Subagent progress update.",
    };
    let mut lines = vec![
        heading.to_string(),
        format!("Run: {}", metadata.run_id),
        format!("Agent: {}", metadata.agent),
        format!("Child index: {}", metadata.index),
    ];
    if let Some(name) = &metadata.session_name {
        lines.push(format!("Child intercom target: {name}"));
    }
    lines.push(String::new());
    lines.push(message.to_string());
    lines.join("\n")
}

/// The three `formatChildOrchestratorMessage` headings (`index.ts:105-110`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildMessageKind {
    /// `need_decision` — "Subagent needs a supervisor decision."
    Ask,
    /// `progress_update` — "Subagent progress update."
    Update,
    /// `interview_request` — "Subagent requests a structured supervisor interview."
    Interview,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn metadata_present_only_when_all_required_vars_set() {
        let full = env_of(&[
            (ENV_ORCH_TARGET, "supervisor"),
            (ENV_RUN_ID, "run-1"),
            (ENV_CHILD_AGENT, "researcher"),
            (ENV_CHILD_INDEX, "0"),
        ]);
        let meta = read_child_orchestrator_metadata_from(full).expect("all required present");
        assert_eq!(meta.orchestrator_target, "supervisor");
        assert_eq!(meta.run_id, "run-1");
        assert_eq!(meta.agent, "researcher");
        assert_eq!(meta.index, "0");
        assert!(meta.orchestrator_session_id.is_none());

        // Missing child index → None.
        let partial = env_of(&[
            (ENV_ORCH_TARGET, "supervisor"),
            (ENV_RUN_ID, "run-1"),
            (ENV_CHILD_AGENT, "researcher"),
        ]);
        assert!(read_child_orchestrator_metadata_from(partial).is_none());
    }

    #[test]
    fn orchestrator_session_id_falls_back_to_intercom_session_id() {
        let env = env_of(&[
            (ENV_ORCH_TARGET, "supervisor"),
            (ENV_RUN_ID, "run-1"),
            (ENV_CHILD_AGENT, "researcher"),
            (ENV_CHILD_INDEX, "2"),
            (ENV_INTERCOM_SESSION_ID, "sess-abc"),
        ]);
        let meta = read_child_orchestrator_metadata_from(env).expect("present");
        assert_eq!(meta.orchestrator_session_id.as_deref(), Some("sess-abc"));
        assert_eq!(preferred_supervisor_target(&meta), "sess-abc");
    }

    #[test]
    fn presence_name_uses_alias_for_unnamed() {
        assert_eq!(presence_name(Some("  Alice "), "id"), "Alice");
        assert_eq!(presence_name(None, "session-deadbeefcafef00d"), "subagent-chat-deadbeef");
        assert_eq!(presence_name(Some("   "), "abcdefghij"), "subagent-chat-abcdefgh");
    }

    #[test]
    fn format_message_includes_run_agent_index_and_body() {
        let meta = ChildOrchestratorMetadata {
            orchestrator_target: "supervisor".to_string(),
            orchestrator_session_id: None,
            run_id: "run-1".to_string(),
            agent: "researcher".to_string(),
            index: "0".to_string(),
            session_name: Some("subagent-chat-1".to_string()),
        };
        let body = format_child_orchestrator_message(ChildMessageKind::Ask, &meta, "Which DB?");
        assert!(body.starts_with("Subagent needs a supervisor decision."));
        assert!(body.contains("Run: run-1"));
        assert!(body.contains("Agent: researcher"));
        assert!(body.contains("Child index: 0"));
        assert!(body.contains("Child intercom target: subagent-chat-1"));
        assert!(body.ends_with("Which DB?"));
    }
}
