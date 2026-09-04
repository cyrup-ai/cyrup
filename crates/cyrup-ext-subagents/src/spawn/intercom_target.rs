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
//! `cyrup-intercom::identity`, and each is a port of its OWN upstream function — which is why
//! `orchestrator_presence_target` is NOT byte-identical to `cyrup_intercom::identity::presence_name`
//! (ICOM-040: pi-subagents v0.47.1 slices 8, pi-intercom v0.10.1 slices 18). See the
//! `[CYRUP-DELTA]` on that function; the `tests` below pin both formulas AND their disagreement.
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
/// `runs/shared/pi-args.ts:16,221` @v0.34.0): the supervisor's addressable presence target the child's
/// `contact_supervisor` relays to. Read by `cyrup-intercom::identity::ENV_ORCH_TARGET`.
pub const ENV_ORCHESTRATOR_TARGET: &str = "CYRUP_SUBAGENT_ORCHESTRATOR_TARGET";
/// `CYRUP_INTERCOM_SESSION_ID` (pi `PI_INTERCOM_SESSION_ID_ENV`,
/// `intercom/intercom-bridge.ts:19` @v0.47.1): the intercom identity override
/// `resolveIntercomSessionTarget` prefers over the passed session id when deriving the
/// unnamed-session alias (`:61,64`). Mirrors `cyrup-intercom::identity::ENV_INTERCOM_SESSION_ID`
/// (the two crates cannot import each other's constants — see the module doc).
pub const ENV_INTERCOM_SESSION_ID: &str = "CYRUP_INTERCOM_SESSION_ID";
/// `CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID` (pi `PI_SUBAGENT_ORCHESTRATOR_SESSION_ID`): the
/// supervisor's stable session id, preferred over the target when set. Read by
/// `cyrup-intercom::identity::ENV_ORCH_SESSION_ID`.
///
/// The spawn site DOES set it, from the launching session's own id: upstream added
/// `env[SUBAGENT_ORCHESTRATOR_SESSION_ID_ENV] = input.parentSessionId` in `3ac0ef5` ("Make
/// supervisor coordination native", `runs/shared/pi-args.ts:221-223`), because the NATIVE supervisor channel
/// keys every request on it — `requestMatchesContext` compares it against
/// `ctx.sessionManager.getSessionId()` so a request only ever surfaces in the session that spawned
/// the child (`native-supervisor-channel.ts:445-448`). An earlier revision of this doc said the
/// spawn site leaves it unset; that was true only at pre-`3ac0ef5` upstream.
pub const ENV_ORCHESTRATOR_SESSION_ID: &str = "CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID";
/// `CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR` (pi `PI_SUBAGENT_SUPERVISOR_CHANNEL_DIR`,
/// `runs/shared/pi-args.ts:18,80-86,225-231` @v0.34.0, added in `3ac0ef5` "Make supervisor coordination native"): the
/// per-child directory holding the NATIVE supervisor channel's `requests/`+`replies/` JSON files.
/// Written by the spawn site alongside [`ENV_ORCHESTRATOR_SESSION_ID`] whenever an orchestrator
/// target, a parent session id, a run id and a persona name are ALL known — upstream's exact
/// four-way condition (`runs/shared/pi-args.ts:226` @v0.34.0). Read by [`crate::native_supervisor::read_child_metadata`].
pub const ENV_SUPERVISOR_CHANNEL_DIR: &str = "CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR";
/// `CYRUP_SUBAGENT_CHILD_AGENT` (pi `PI_SUBAGENT_CHILD_AGENT`, `runs/shared/pi-args.ts:86,736`): the child's
/// own persona name (one of the four vars required for the metadata gate to activate). Read by
/// `cyrup-intercom::identity::ENV_CHILD_AGENT`.
pub const ENV_CHILD_AGENT: &str = "CYRUP_SUBAGENT_CHILD_AGENT";
/// `CYRUP_SUBAGENT_INTERCOM_SESSION_NAME` (pi `PI_SUBAGENT_INTERCOM_SESSION_NAME`,
/// `runs/shared/pi-args.ts:703-705`): the child's OWN deterministic presence label
/// ([`resolve_subagent_intercom_target`]) — the addressable name the parent steers. Read by
/// `cyrup-intercom::identity::ENV_INTERCOM_SESSION_NAME`.
pub const ENV_INTERCOM_SESSION_NAME: &str = "CYRUP_SUBAGENT_INTERCOM_SESSION_NAME";

/// pi `sanitizeIntercomTargetPart` (`intercom/intercom-bridge.ts:69-71`): lowercase, collapse every run of
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

/// pi `resolveSubagentIntercomTarget` (`intercom/intercom-bridge.ts:73-76`): the deterministic broker
/// presence label a spawned child registers under (and the parent addresses to steer it) —
/// `subagent-<sanitize(agent)>-<sanitize(run_id)>-<index+1>`.
#[must_use]
pub fn resolve_subagent_intercom_target(run_id: &str, agent: &str, index: usize) -> String {
    resolve_subagent_intercom_target_opt(run_id, agent, Some(index))
}

/// The index-OPTIONAL form of [`resolve_subagent_intercom_target`], matching upstream's signature
/// exactly: `resolveSubagentIntercomTarget(runId, agent, index?: number)` renders
/// `stepSuffix = index !== undefined ? `-${index + 1}` : ""` (`intercom/intercom-bridge.ts:94-97`
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

/// pi `resolveIntercomSessionTarget` (`pi-subagents/src/intercom/intercom-bridge.ts:61-67`
/// @v0.47.1): a supervisor's own addressable presence target — the trimmed session name if present,
/// else the unnamed-session alias `subagent-chat-<id[0:8]>` over the `PI_INTERCOM_SESSION_ID` env
/// value when set and non-blank, otherwise `sessionId` (with a leading `session-` prefix stripped
/// first).
///
/// ```ts
/// export function resolveIntercomSessionTarget(sessionName, sessionId, intercomSessionId = process.env[PI_INTERCOM_SESSION_ID_ENV]): string {
///   const trimmedName = sessionName?.trim();
///   if (trimmedName) return trimmedName;
///   const fallbackSessionId = intercomSessionId?.trim() || sessionId;
///   const normalizedSessionId = fallbackSessionId.startsWith("session-") ? fallbackSessionId.slice("session-".length) : fallbackSessionId;
///   return `${DEFAULT_INTERCOM_TARGET_PREFIX}-${normalizedSessionId.slice(0, 8)}`;
/// }
/// ```
///
/// ## [CYRUP-DELTA] — this is NOT byte-identical to `cyrup_intercom::identity::presence_name`
///
/// ICOM-040. This function's doc used to claim byte-identity with
/// `cyrup_intercom::identity::presence_name`, and that claim was false: `presence_name` takes
/// **18** id characters (pi-intercom `resolveIntercomPresenceName`, `pi-intercom/index.ts:419-425`
/// @v0.10.1, widened from 8 at v0.10.0 because two UUIDv7 ids minted in the same millisecond share
/// far more than 8 leading characters), while this one takes **8**.
///
/// The divergence is UPSTREAM'S, not cyrup's: pi-subagents v0.47.1 still slices at 8 while
/// pi-intercom v0.10.1 slices at 18, so an UNNAMED supervisor is registered under a different
/// string from the one it hands its children. Widening this to 18 would fix the symptom and break
/// parity, so it stays at 8 and the disagreement is pinned by
/// `the_two_presence_derivations_disagree_for_an_unnamed_session` below.
///
/// What bounds the blast radius in cyrup is that the alias is only the FALLBACK: the spawn path
/// also writes `CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID` (`exec/mod.rs`) and
/// `preferred_supervisor_target` prefers it, so a child addresses its supervisor by id.
#[must_use]
pub fn orchestrator_presence_target(session_name: Option<&str>, session_id: &str) -> String {
    orchestrator_presence_target_with(
        session_name,
        session_id,
        std::env::var(ENV_INTERCOM_SESSION_ID).ok().as_deref(),
    )
}

/// [`orchestrator_presence_target`] with the `PI_INTERCOM_SESSION_ID` value supplied explicitly —
/// upstream's third parameter, which defaults to `process.env[PI_INTERCOM_SESSION_ID_ENV]`
/// (`intercom-bridge.ts:61`). Split out so the env-fallback rung is testable without mutating the
/// process environment (this crate is `edition 2024`, where `set_var` is `unsafe`).
#[must_use]
pub fn orchestrator_presence_target_with(
    session_name: Option<&str>,
    session_id: &str,
    intercom_session_id: Option<&str>,
) -> String {
    if let Some(name) = session_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    // `const fallbackSessionId = intercomSessionId?.trim() || sessionId;` (`:64`) — a blank env
    // value is falsy in JS and must fall through to `sessionId`, not produce `subagent-chat-`.
    let fallback = intercom_session_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(session_id);
    let normalized = fallback.strip_prefix("session-").unwrap_or(fallback);
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
        assert_eq!(
            ENV_ORCHESTRATOR_TARGET,
            "CYRUP_SUBAGENT_ORCHESTRATOR_TARGET"
        );
        assert_eq!(ENV_INTERCOM_SESSION_ID, "CYRUP_INTERCOM_SESSION_ID");
        assert_eq!(
            ENV_ORCHESTRATOR_SESSION_ID,
            "CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID"
        );
        assert_eq!(
            ENV_SUPERVISOR_CHANNEL_DIR,
            "CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR"
        );
        assert_eq!(ENV_CHILD_AGENT, "CYRUP_SUBAGENT_CHILD_AGENT");
        assert_eq!(
            ENV_INTERCOM_SESSION_NAME,
            "CYRUP_SUBAGENT_INTERCOM_SESSION_NAME"
        );
        assert_eq!(RUN_ID_ENV, "CYRUP_SUBAGENT_RUN_ID");
        assert_eq!(CHILD_INDEX_ENV, "CYRUP_SUBAGENT_CHILD_INDEX");
    }

    #[test]
    fn sanitize_matches_pi_regex_behavior() {
        // trim + lowercase + non-[a-z0-9_-] runs -> single '-' + strip edge '-'.
        assert_eq!(
            sanitize_intercom_target_part("  Research Bot  "),
            "research-bot"
        );
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

    /// `resolveIntercomSessionTarget` (`intercom-bridge.ts:61-67` @v0.47.1): a trimmed name wins,
    /// else `subagent-chat-<id[0:8]>` with `session-` stripped.
    #[test]
    fn orchestrator_presence_target_is_name_else_eight_char_alias() {
        assert_eq!(
            orchestrator_presence_target_with(Some("  Alice "), "id", None),
            "Alice"
        );
        assert_eq!(
            orchestrator_presence_target_with(None, "session-deadbeefcafef00d", None),
            "subagent-chat-deadbeef"
        );
        assert_eq!(
            orchestrator_presence_target_with(Some("   "), "abcdefghij", None),
            "subagent-chat-abcdefgh"
        );
    }

    /// ICOM-040 — the `PI_INTERCOM_SESSION_ID` rung: `intercomSessionId?.trim() || sessionId`
    /// (`intercom-bridge.ts:64`). A SET, non-blank env value OUTRANKS the passed session id; a
    /// blank one is JS-falsy and must fall through.
    ///
    /// RED before this pass: `orchestrator_presence_target` took only `(name, id)` and never
    /// consulted the env at all, so a session whose intercom identity was pinned by
    /// `CYRUP_INTERCOM_SESSION_ID` handed its children an alias derived from the WRONG id — the
    /// child then addressed a presence name the supervisor had never registered.
    #[test]
    fn a_set_intercom_session_id_env_outranks_the_session_id() {
        assert_eq!(
            orchestrator_presence_target_with(
                None,
                "session-aaaaaaaabbbb",
                Some("session-ccccccccdddd")
            ),
            "subagent-chat-cccccccc"
        );
        // `?.trim() ||` — blank is falsy, so it falls through to `sessionId`.
        assert_eq!(
            orchestrator_presence_target_with(None, "session-aaaaaaaabbbb", Some("   ")),
            "subagent-chat-aaaaaaaa"
        );
    }

    /// ICOM-040 — pins the UPSTREAM divergence this function's doc used to deny, so that a future
    /// pass cannot "fix" it into a parity break.
    ///
    /// pi-subagents `resolveIntercomSessionTarget` slices 8 (`intercom-bridge.ts:66` @v0.47.1);
    /// pi-intercom `resolveIntercomPresenceName` slices 18 (`pi-intercom/index.ts:424` @v0.10.1,
    /// widened at v0.10.0). Both are ported literally, so for an UNNAMED session the two strings
    /// differ — upstream's own bug, and the reason the supervisor address is carried by
    /// `CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID` rather than by this alias.
    ///
    /// RED before this pass: nothing asserted the relationship; the doc asserted the OPPOSITE of
    /// what the code does.
    #[test]
    fn the_two_presence_derivations_disagree_for_an_unnamed_session() {
        let id = "session-0193f7a2b1c84e6f9012";
        let subagent_side = orchestrator_presence_target_with(None, id, None);
        // What `cyrup_intercom::identity::presence_name` produces for the same input. Spelled out
        // rather than called: `cyrup-intercom` depends on THIS crate, so the edge forbids the
        // import (module doc). `cyrup-intercom`'s own `presence_name_uses_alias_for_unnamed` pins
        // the same formula from the other side.
        let intercom_side = "subagent-chat-0193f7a2b1c84e6f90";
        assert_eq!(subagent_side, "subagent-chat-0193f7a2");
        assert_ne!(
            subagent_side, intercom_side,
            "8 vs 18 — pi-subagents v0.47.1 and pi-intercom v0.10.1 disagree, and cyrup ports both \
             literally; see the [CYRUP-DELTA] on `orchestrator_presence_target`"
        );
        // A NAMED session agrees on both sides (both return the trimmed name), which is the case
        // that actually has to work.
        assert_eq!(
            orchestrator_presence_target_with(Some(" Alice "), id, None),
            "Alice"
        );
    }
}
