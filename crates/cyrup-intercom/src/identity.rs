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
/// `CYRUP_INTERCOM_STABLE_ID` (pi `STABLE_INTERCOM_SESSION_ID_ENV = "PI_INTERCOM_STABLE_ID"`,
/// `v0.10.1 index.ts:42`), resolved by `resolveConfiguredIntercomSessionId`
/// (`v0.10.1 index.ts:434-436`) and consumed at register.
///
/// Distinct from [`ENV_INTERCOM_SESSION_ID`] in BOTH direction and meaning: that one is *published*
/// downward for a child to read back as its supervisor's id, this one is *read* by a session as its
/// own restart-stable registration id.
pub const ENV_INTERCOM_STABLE_ID: &str = "CYRUP_INTERCOM_STABLE_ID";
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
/// `CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR` (pi `SUBAGENT_SUPERVISOR_CHANNEL_DIR_ENV`,
/// `v0.10.1 index.ts:44`) — set by the launcher on every child it hands a **native** supervisor
/// channel. Its presence suppresses the legacy broker-routed `contact_supervisor` tool
/// (`v0.10.1 index.ts:1505-1507`); the v0.7.0 CHANGELOG names it: "suppressing legacy supervisor
/// tools when native supervisor channels are present".
///
/// Written on cyrup's production spawn path by `cyrup-ext-subagents`
/// (`spawn/intercom_target.rs::ENV_SUPERVISOR_CHANNEL_DIR`, inserted at `exec/mod.rs`'s child env
/// overlay and consumed by `native_supervisor.rs`) — the same variable, declared here because this
/// crate must not depend on that one's private constants.
pub const ENV_SUPERVISOR_CHANNEL_DIR: &str = "CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR";

/// `const nativeSupervisorChannelAvailable = Boolean(process.env[SUBAGENT_SUPERVISOR_CHANNEL_DIR_ENV]?.trim())`
/// (`v0.10.1 index.ts:1504`). Blank/whitespace counts as absent, exactly as `?.trim()` + `Boolean`.
#[must_use]
pub fn native_supervisor_channel_available_from(env: impl Fn(&str) -> Option<String>) -> bool {
    env(ENV_SUPERVISOR_CHANNEL_DIR).is_some_and(|v| !v.trim().is_empty())
}

/// [`native_supervisor_channel_available_from`] over the process environment.
#[must_use]
pub fn native_supervisor_channel_available() -> bool {
    native_supervisor_channel_available_from(|k| std::env::var(k).ok())
}

/// `getNamePollMs()` (`v0.10.1 index.ts:486-495`, 10 lines):
///
/// ```text
/// const configured = process.env[NAME_POLL_MS_ENV];
/// if (configured !== undefined) {
///   const value = Number(configured);
///   if (Number.isFinite(value) && value > 0) return value;
/// }
/// return 1000;
/// ```
///
/// Note this is NOT `getAskTimeoutMs`'s shape: an invalid value here falls back to the 1000 ms
/// default rather than throwing, and a **fractional** value is accepted (`Number.isFinite`, not
/// `Number.isSafeInteger`). Both differences are upstream's, so both are reproduced —
/// `"1500.5"` polls at 1500 ms after the truncation a `Duration::from_millis` forces.
#[must_use]
pub fn name_poll_ms() -> u64 {
    name_poll_ms_from(|k| std::env::var(k).ok())
}

/// The pure core of [`name_poll_ms`].
#[must_use]
pub fn name_poll_ms_from(env: impl Fn(&str) -> Option<String>) -> u64 {
    const DEFAULT_NAME_POLL_MS: u64 = 1000;
    match env(ENV_INTERCOM_NAME_POLL_MS) {
        // `Number("")` is 0, and `0 > 0` is false, so a blank value takes the default.
        Some(raw) => match raw.trim().parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => v as u64,
            _ => DEFAULT_NAME_POLL_MS,
        },
        None => DEFAULT_NAME_POLL_MS,
    }
}

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

/// `resolveIntercomPresenceName` (`v0.10.1 index.ts:419-426`): the trimmed session name if present,
/// else the unnamed-session alias `subagent-chat-<id[0:18]>` (with a leading `session-` prefix
/// stripped first).
///
/// The slice was **18**, not 8, from v0.10.0 (`126875e`, CHANGELOG 0.10.0: "Extend unnamed-session
/// fallback aliases with enough session-ID characters to distinguish UUIDv7 sessions started close
/// together"). This alias is the presence NAME the session registers under, so two unnamed sessions
/// whose UUIDv7 ids were minted in the same millisecond used to register the SAME name and neither
/// was reachable by it — `broker/routing.rs::find_session_ids` returned both and the broker answered
/// every send with `Multiple sessions named "…" are connected.`
#[must_use]
pub fn presence_name(session_name: Option<&str>, session_id: &str) -> String {
    if let Some(name) = session_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let normalized = session_id.strip_prefix("session-").unwrap_or(session_id);
    let short: String = normalized.chars().take(18).collect();
    format!("{DEFAULT_UNNAMED_SESSION_ALIAS_PREFIX}-{short}")
}

/// `shortSessionId` (`v0.9.2 index.ts:365-367`): the first 8 chars of a session id.
///
/// Upstream **kept** this at 8 for the picker label (`formatSessionLabel`,
/// `v0.10.1 index.ts:440-446`) and replaced it with [`session_id_prefixes`] for the `list` row,
/// which is the addressable column — do not conflate the two.
#[must_use]
pub fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

/// `sessionIdPrefixes` (`v0.10.1 index.ts:387-406`, v0.9.3 `72309e0` "fix: show unique session ID
/// prefixes"): the shortest *distinguishing* id prefix for every session in one roster.
///
/// ```text
/// for (const session of sessions) {
///   let longestSharedPrefix = 0;
///   for (const other of sessions) {
///     if (other.id === session.id) continue;
///     let length = 0;
///     while (length < session.id.length && session.id[length] === other.id[length]) length += 1;
///     longestSharedPrefix = Math.max(longestSharedPrefix, length);
///   }
///   const minimumLength = Math.max(8, longestSharedPrefix + 1);
///   const groupBoundary = session.id.indexOf("-", minimumLength);
///   const length = groupBoundary === -1 ? minimumLength : groupBoundary;
///   prefixes.set(session.id, session.id.slice(0, length));
/// }
/// ```
///
/// Mechanism notes, both load-bearing:
/// - the comparison is by **UTF-16 code unit** upstream (`session.id[length]`) — session ids are
///   UUIDs, so this port compares `char`s and slices on char boundaries, which agrees for every id
///   the broker can mint and never panics on a non-ASCII one.
/// - `indexOf("-", minimumLength)` **extends** the prefix to the next `-` group boundary, and
///   returns the boundary index itself, so the emitted prefix EXCLUDES the `-`. A boundary at or
///   after `minimumLength` therefore always lengthens (never shortens) the prefix.
#[must_use]
pub fn session_id_prefixes<'a, I>(session_ids: I) -> std::collections::HashMap<String, String>
where
    I: IntoIterator<Item = &'a str> + Clone,
{
    let mut prefixes = std::collections::HashMap::new();
    for id in session_ids.clone() {
        let chars: Vec<char> = id.chars().collect();
        let mut longest_shared_prefix = 0usize;
        for other in session_ids.clone() {
            if other == id {
                continue;
            }
            let mut length = 0usize;
            let mut other_chars = other.chars();
            for c in &chars {
                match other_chars.next() {
                    Some(o) if o == *c => length += 1,
                    _ => break,
                }
            }
            longest_shared_prefix = std::cmp::max(longest_shared_prefix, length);
        }
        let minimum_length = std::cmp::max(8, longest_shared_prefix + 1);
        // `String::indexOf(search, position)` starts AT `position`; a `minimum_length` past the end
        // simply finds nothing, which is JS's `-1` branch.
        let group_boundary = chars
            .iter()
            .enumerate()
            .skip(minimum_length)
            .find(|(_, c)| **c == '-')
            .map(|(i, _)| i);
        let length = group_boundary.unwrap_or(minimum_length);
        prefixes.insert(id.to_string(), chars.iter().take(length).collect());
    }
    prefixes
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

    // `v0.10.1 index.ts:425` — `.slice(0, 18)`, NOT `.slice(0, 8)` (v0.10.0 `126875e`).
    #[test]
    fn presence_name_uses_alias_for_unnamed() {
        assert_eq!(presence_name(Some("  Alice "), "id"), "Alice");
        assert_eq!(presence_name(None, "session-deadbeefcafef00d"), "subagent-chat-deadbeefcafef00d");
        assert_eq!(presence_name(Some("   "), "abcdefghij"), "subagent-chat-abcdefghij");
    }

    /// The whole point of v0.10.0's 8→18 widening (`126875e`, CHANGELOG 0.10.0: "Extend
    /// unnamed-session fallback aliases with enough session-ID characters to distinguish UUIDv7
    /// sessions started close together").
    ///
    /// A UUIDv7 spends its first 13 hex digits on the 48-bit unix-ms timestamp plus the version
    /// nibble, so two ids minted in the SAME millisecond are byte-identical through
    /// `0192f3c1-9a10-7`. The first field that can differ is `rand_a`, the 12 random bits filling
    /// out the third group — characters 15..18. `slice(0, 8)` stops inside the timestamp, so both
    /// sessions registered under one presence name and neither was addressable; `slice(0, 18)`
    /// reaches past the version nibble and takes all of `rand_a`, which is what separates them.
    ///
    /// The fixture therefore has to vary `rand_a` (`7a3c` vs `7f21`) — that is what a real
    /// same-millisecond mint produces. Ids differing only in the trailing random block would still
    /// collide at 18 and would pin nothing about the widening.
    #[test]
    fn unnamed_aliases_distinguish_uuidv7_ids_minted_in_the_same_millisecond() {
        let a = "0192f3c1-9a10-7a3c-8000-aaaaaaaaaaaa";
        let b = "0192f3c1-9a10-7f21-8000-bbbbbbbbbbbb";
        // Same mint millisecond: identical through the timestamp AND the version nibble.
        assert_eq!(&a[..15], &b[..15], "the fixture must be two same-millisecond UUIDv7 mints");
        // …so the pre-fix 8-char alias could not tell them apart.
        assert_eq!(&a[..8], &b[..8], "slice(0, 8) lands inside the shared timestamp");

        assert_ne!(presence_name(None, a), presence_name(None, b));
        assert_eq!(presence_name(None, a), "subagent-chat-0192f3c1-9a10-7a3c");
        assert_eq!(presence_name(None, b), "subagent-chat-0192f3c1-9a10-7f21");
    }

    /// `v0.10.1 index.ts:387-406`. Two ids sharing 20 characters must get prefixes that differ, are
    /// at least 8 chars, and are extended to (but excluding) the next `-` group boundary.
    #[test]
    fn session_id_prefixes_are_distinguishing_and_group_aligned() {
        let a = "0192f3c1-9a10-7000-8000-aaaaaaaaaaaa";
        let b = "0192f3c1-9a10-7000-8000-bbbbbbbbbbbb";
        let map = session_id_prefixes([a, b]);
        let pa = map.get(a).expect("a present");
        let pb = map.get(b).expect("b present");
        assert_ne!(pa, pb, "prefixes must distinguish");
        // Longest shared prefix is 24 chars ("0192f3c1-9a10-7000-8000-"); minimum is 25; the next
        // `-` at or after index 25 does not exist, so the prefix is exactly 25 chars.
        assert_eq!(pa, "0192f3c1-9a10-7000-8000-a");
        assert_eq!(pb, "0192f3c1-9a10-7000-8000-b");
    }

    /// The `Math.max(8, …)` floor and the group-boundary extension, on ids that share nothing.
    #[test]
    fn session_id_prefixes_floor_at_eight_then_extend_to_the_group_boundary() {
        // Distinct from character 0, so `longestSharedPrefix == 0` → minimumLength 8; the next `-`
        // is at index 8, so the prefix is the first 8 chars (the `-` is excluded).
        let map = session_id_prefixes(["aaaaaaaa-1111-2222", "bbbbbbbb-1111-2222"]);
        assert_eq!(map.get("aaaaaaaa-1111-2222").map(String::as_str), Some("aaaaaaaa"));

        // A lone session has no `other`, so it also floors at 8.
        let solo = session_id_prefixes(["0192f3c1-9a10-7000-8000-aaaaaaaaaaaa"]);
        assert_eq!(solo.get("0192f3c1-9a10-7000-8000-aaaaaaaaaaaa").map(String::as_str), Some("0192f3c1"));

        // An id SHORTER than the 8-char floor with no `-` yields the whole id, not a panic.
        let short = session_id_prefixes(["ab", "cd"]);
        assert_eq!(short.get("ab").map(String::as_str), Some("ab"));
    }

    /// ICOM-030 / `v0.10.1 index.ts:1504`:
    /// `Boolean(process.env[SUBAGENT_SUPERVISOR_CHANNEL_DIR_ENV]?.trim())`.
    #[test]
    fn native_supervisor_channel_probe_treats_blank_as_absent() {
        assert!(!native_supervisor_channel_available_from(|_| None));
        assert!(!native_supervisor_channel_available_from(|_| Some(String::new())));
        assert!(!native_supervisor_channel_available_from(|_| Some("   ".to_string())));
        assert!(native_supervisor_channel_available_from(|k| (k == ENV_SUPERVISOR_CHANNEL_DIR)
            .then(|| "/tmp/chan".to_string())));
    }

    /// `getNamePollMs()` (`v0.10.1 index.ts:486-495`). Unlike `getAskTimeoutMs`, an invalid value
    /// FALLS BACK rather than throwing, and a fractional value is accepted (`Number.isFinite`).
    #[test]
    fn name_poll_ms_falls_back_rather_than_erroring() {
        assert_eq!(name_poll_ms_from(|_| None), 1000);
        assert_eq!(name_poll_ms_from(|_| Some(String::new())), 1000);
        assert_eq!(name_poll_ms_from(|_| Some("abc".to_string())), 1000);
        assert_eq!(name_poll_ms_from(|_| Some("0".to_string())), 1000);
        assert_eq!(name_poll_ms_from(|_| Some("-5".to_string())), 1000);
        assert_eq!(name_poll_ms_from(|_| Some("250".to_string())), 250);
        assert_eq!(name_poll_ms_from(|_| Some("1500.5".to_string())), 1500);
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
