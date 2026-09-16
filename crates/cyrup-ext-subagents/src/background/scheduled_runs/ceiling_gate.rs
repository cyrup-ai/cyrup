//! The create-time capability-ceiling gate: whether a session is ALLOWED to persist a schedule at
//! all.
//!
//! Ports pi `scheduled-runs.ts:619-620` @ `7fe9dee1`:
//!
//! ```ts
//! const sessionId = ctx.sessionManager.getSessionId() ?? "unknown";
//! if (this.deps.resolveCapabilityCeiling?.(sessionId))
//!     return textResult("Cannot persist a schedule while a capability ceiling is active.", undefined, undefined, true);
//! ```
//!
//! # Why a schedule is different from every other thing a ceilinged session may do
//!
//! A [`ResolvedCapabilityCeiling`] is a bound on a RUNNING subtree: it is registered for a
//! session, it is intersected into every child that session launches, and it disappears when the
//! handle that registered it drops. A schedule is the one artefact that OUTLIVES all of that. A
//! ceilinged session that could persist a schedule would be minting authority for a future
//! process that has no ceiling registered at all — the ceiling would be escaped simply by waiting.
//! That is why the refusal is absolute and why it does not inspect the ceiling's axes.
//!
//! # THE TRAP — this gates CREATE, and only create
//!
//! Upstream calls it at exactly ONE site (`create`, `:620`). `pause` (`:678`), `remove` (`:715`),
//! `launch` (`:819`, `:839`, `:849`, `:867`), `finishRun` (`:898`) and `recordMissed` (`:916`)
//! all write to the store with no ceiling check whatsoever.
//!
//! **If this gate were applied inside [`super::store::ScheduleStore::write`], the store would
//! deadlock.** A run fired before a ceiling was registered completes while the ceiling is active;
//! `finishRun` can no longer clear `activeRunId`; `active.lock` is never removed; and the schedule
//! is permanently wedged in the running state with `overlap: "skip"` refusing every future fire.
//! The ceiling's purpose is to stop new authority being MINTED, not to stop existing authority
//! being WOUND DOWN.
//!
//! That is the whole reason this is a free function in its own file that the CREATE path calls,
//! rather than a guard inside the writer where a later reader would inevitably "tighten" it.

use crate::exec::capability_ceiling::{
    CAPABILITY_CEILING_ENV, CAPABILITY_CEILING_VERSION, ResolvedCapabilityCeiling,
    decode_capability_ceiling, resolve_capability_ceiling,
};
use crate::identity::SessionId;

/// pi `:619`'s literal `"unknown"`.
pub const UNKNOWN_SESSION_KEY: &str = "unknown";

/// pi `:620`'s refusal sentence, byte-verbatim. It is CONTRACT: a host that matches on it, and an
/// agent that has learned what it means, both depend on the exact bytes.
pub const SCHEDULE_CEILING_REFUSAL: &str =
    "Cannot persist a schedule while a capability ceiling is active.";

/// The `sources` entry [`process_ceiling_resolver`] stamps on the synthetic ceiling it returns
/// when the inherited value is malformed, so an operator reading a diagnostic can tell the
/// fail-closed path apart from a genuinely registered ceiling.
pub const MALFORMED_CEILING_SOURCE: &str = "malformed-inherited-capability-ceiling";

/// pi's optional `resolveCapabilityCeiling?:` dep (`:86`), as an explicit seam.
///
/// Upstream makes the dep OPTIONAL so its own tests run with no gate at all. A port that copied
/// that would let any caller disable the gate by omission, so this is a required parameter and
/// the "no gate" case is spelled by passing a resolver that returns [`None`] — which a test does
/// deliberately and production never can, because production passes
/// [`process_ceiling_resolver`].
///
/// `+ Sync` since SUBA-016 part B: the tool surface holds this reference across `.await` points
/// (the store is async), and `&dyn Fn` is only `Send` when the referent is `Sync`. Every caller
/// already satisfies it — a plain `fn` item is `Sync`, and so is any closure over `Sync` captures.
pub type CeilingResolver<'a> = &'a (dyn Fn(&str) -> Option<ResolvedCapabilityCeiling> + Sync);

/// pi `:619` — `ctx.sessionManager.getSessionId() ?? "unknown"`.
///
/// # Here, `None` is a KEY, not an exemption
///
/// `executor/session_state.rs:52-53` documents the crate-wide meaning of an absent session:
/// *"no filter, no stamp"*. **This function means the opposite.** A headless, unpersisted or
/// host-service-less process that has no session identity does not thereby become exempt from the
/// ceiling; it is looked up under the literal key `"unknown"`, which is exactly the key such a
/// host registers a ceiling under. A future "consistency" refactor that turned this back into an
/// `Option` pass-through would invert the guarantee for precisely the hosts least able to notice.
#[must_use]
pub fn ceiling_lookup_key(session_id: Option<&SessionId>) -> &str {
    session_id.map_or(UNKNOWN_SESSION_KEY, SessionId::as_str)
}

/// pi `:619-620`. `Some(refusal)` means the caller MUST NOT persist the schedule.
///
/// The check is `is_some()` on the resolved ceiling — **any** ceiling, of any shape, including one
/// whose `allowed_tools` and `allowed_agents` are both `None` and whose `deny_extensions` is
/// `false`. Upstream's `:620` is a bare truthiness test on the returned object and deliberately
/// does not inspect the axes; `intersect_capability_ceilings`
/// ([`crate::exec::capability_ceiling`]) already returns [`None`] when nothing is registered, so
/// the PRESENCE of a value IS the restriction.
#[must_use]
pub fn schedule_persistence_refusal(
    session_id: Option<&SessionId>,
    resolve: CeilingResolver<'_>,
) -> Option<&'static str> {
    resolve(ceiling_lookup_key(session_id)).map(|_| SCHEDULE_CEILING_REFUSAL)
}

/// The PRODUCTION resolver — the inherited env ceiling intersected with the process-local
/// registry, **fail-closed**.
///
/// This is [`ceiling_resolver_from`] instantiated with the real process environment, following
/// the crate's [`crate::background::temp_root_dir`]/`temp_root_dir_from` convention: the ambient
/// input is a parameter of the pure core so both branches are provable without mutating
/// process-global state (this crate is `#![forbid(unsafe_code)]` and edition 2024 makes
/// `std::env::set_var` `unsafe`, so a test cannot pin the variable).
#[must_use]
pub fn process_ceiling_resolver(session_key: &str) -> Option<ResolvedCapabilityCeiling> {
    ceiling_resolver_from(session_key, &|key| std::env::var_os(key))
}

/// The pure core of [`process_ceiling_resolver`] — pi
/// `resolveCurrentSubagentCapabilityCeiling` (`capability-ceiling.ts:168-170`) with its one
/// ambient input injected, and with the `Err` arm decided.
///
/// [`crate::exec::capability_ceiling::resolve_current_capability_ceiling`]'s own doc states the rule this implements: *"a
/// MALFORMED ceiling must fail loudly rather than degrade to 'unbounded', which would invert the
/// guarantee."* A malformed [`CAPABILITY_CEILING_ENV`] therefore resolves to a ceiling here — the
/// most restrictive one expressible (both axes bound to nothing, extensions denied), stamped
/// [`MALFORMED_CEILING_SOURCE`] — so the schedule is refused. Degrading the error to [`None`]
/// would let a corrupt env value BUY the ability to persist a schedule, which is strictly worse
/// than a refusal.
///
/// The caller still surfaces [`SCHEDULE_CEILING_REFUSAL`] and not a decode diagnostic: a schedule
/// create is not the place to explain an env-var encoding problem, and the two outcomes are
/// identical to the user — the schedule was not written.
#[must_use]
pub fn ceiling_resolver_from(
    session_key: &str,
    env: crate::paths::EnvLookup<'_>,
) -> Option<ResolvedCapabilityCeiling> {
    let raw = env(CAPABILITY_CEILING_ENV).and_then(|value| value.into_string().ok());
    match decode_capability_ceiling(raw.as_deref()) {
        Ok(inherited) => resolve_capability_ceiling(Some(session_key), inherited),
        Err(_) => Some(ResolvedCapabilityCeiling {
            version: CAPABILITY_CEILING_VERSION,
            allowed_tools: Some(Vec::new()),
            allowed_agents: Some(Vec::new()),
            deny_extensions: true,
            sources: vec![MALFORMED_CEILING_SOURCE.to_string()],
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::ffi::OsString;

    use super::super::store::ScheduleStore;
    use super::super::test_fixtures::full_record;
    use super::*;
    use crate::exec::capability_ceiling::register_capability_ceiling;

    /// An env table with nothing in it — the production state of
    /// [`CAPABILITY_CEILING_ENV`] on an orchestrator that never inherited a ceiling.
    fn no_env(_key: &str) -> Option<OsString> {
        None
    }

    fn session(raw: &str) -> SessionId {
        SessionId::parse(raw).expect("test session id is non-empty")
    }

    /// pi `:620`. The refusal text is CONTRACT and is asserted byte-for-byte against the sentence
    /// in upstream's `textResult(...)` call, not against the constant that defines it.
    #[test]
    fn a_schedule_cannot_be_persisted_under_an_active_capability_ceiling() {
        assert_eq!(
            SCHEDULE_CEILING_REFUSAL,
            "Cannot persist a schedule while a capability ceiling is active."
        );

        let sid = session("ceiling-gate-refuses");
        // The handle MUST stay in a live binding: `let _ = register(...)` drops it immediately
        // (`CapabilityCeilingHandle: Drop` disposes), the ceiling never exists, and this test
        // would pass by not testing anything.
        let _handle = register_capability_ceiling(
            sid.as_str(),
            "org-policy",
            &serde_json::json!({ "allowedAgents": ["reviewer"] }),
        )
        .expect("registers");

        assert_eq!(
            schedule_persistence_refusal(Some(&sid), &|key| ceiling_resolver_from(key, &no_env)),
            Some(SCHEDULE_CEILING_REFUSAL),
        );
        assert_eq!(
            schedule_persistence_refusal(Some(&sid), &process_ceiling_resolver),
            Some(SCHEDULE_CEILING_REFUSAL),
            "the production resolver must reach the same registry"
        );
    }

    /// Any ceiling refuses, including one that bounds NOTHING — upstream's `:620` is a bare
    /// truthiness test on the resolved object and deliberately does not inspect the axes.
    #[test]
    fn a_ceiling_that_bounds_nothing_still_refuses() {
        let sid = session("ceiling-gate-empty-ceiling");
        let _handle = register_capability_ceiling(
            sid.as_str(),
            "org-policy",
            &serde_json::json!({ "denyExtensions": false }),
        )
        .expect("registers");

        let resolved = ceiling_resolver_from(sid.as_str(), &no_env).expect("a ceiling is present");
        assert_eq!(resolved.allowed_tools, None);
        assert_eq!(resolved.allowed_agents, None);
        assert!(!resolved.deny_extensions);
        assert_eq!(
            schedule_persistence_refusal(Some(&sid), &|key| ceiling_resolver_from(key, &no_env)),
            Some(SCHEDULE_CEILING_REFUSAL),
        );
    }

    /// The negative, and the proof the gate does not LATCH: once the handle is dropped the same
    /// session persists normally, and the store write it was blocking succeeds.
    #[tokio::test]
    async fn a_schedule_persists_normally_with_no_ceiling() {
        let sid = session("ceiling-gate-releases");
        {
            let _handle = register_capability_ceiling(
                sid.as_str(),
                "org-policy",
                &serde_json::json!({ "allowedTools": ["read"] }),
            )
            .expect("registers");
            assert_eq!(
                schedule_persistence_refusal(Some(&sid), &process_ceiling_resolver),
                Some(SCHEDULE_CEILING_REFUSAL),
            );
        }

        assert_eq!(
            schedule_persistence_refusal(Some(&sid), &process_ceiling_resolver),
            None,
            "the gate must not latch once the registration is disposed"
        );

        let project = tempfile::tempdir().expect("real tempdir");
        let root = super::super::scheduled_run_store_path(project.path(), Some(&sid), None);
        let store = ScheduleStore::new(root, Some(project.path().to_path_buf()));
        let record = full_record("after-ceiling", project.path());
        store.write(&record).await.expect("write succeeds");
        assert_eq!(
            store.get(&record.id).await.expect("reads back").id,
            record.id
        );
    }

    /// pi `:619`'s `?? "unknown"`. A host with NO session identity is looked up under the literal
    /// key `"unknown"` — the key such a host registers a ceiling under — and is therefore BOUND.
    ///
    /// The natural Rust spelling (`resolve_capability_ceiling(session_id, ..)`, passing the
    /// `Option` straight through) compiles, reads as "no session", and skips the registry lookup
    /// entirely, so a headless host would be UNBOUND while a named one is bound. That inversion
    /// is what this test exists to catch, which is why it asserts through the registry and not
    /// merely that some value came back.
    #[test]
    fn a_host_with_no_session_uses_the_unknown_key() {
        assert_eq!(ceiling_lookup_key(None), "unknown");
        assert_eq!(ceiling_lookup_key(None), UNKNOWN_SESSION_KEY);
        assert_eq!(
            ceiling_lookup_key(Some(&session("named"))),
            "named",
            "a real session is not overwritten by the fallback"
        );

        let _handle = register_capability_ceiling(
            UNKNOWN_SESSION_KEY,
            "org-policy",
            &serde_json::json!({ "denyExtensions": true }),
        )
        .expect("registers");

        assert_eq!(
            schedule_persistence_refusal(None, &process_ceiling_resolver),
            Some(SCHEDULE_CEILING_REFUSAL),
            "a host with no session must be bound by the ceiling registered under 'unknown'"
        );
        assert_eq!(
            schedule_persistence_refusal(
                Some(&session("some-other-session")),
                &process_ceiling_resolver
            ),
            None,
            "and the 'unknown' registration must not leak onto a named session"
        );
    }

    /// §4.4 — the gate is a CREATE-time gate. Winding an existing schedule down (clearing
    /// `activeRunId`, appending a `finishRun` event, pausing, deleting) must keep working while a
    /// ceiling is active, or a run in flight when the ceiling is registered wedges the schedule
    /// forever: `activeRunId` is never cleared, `active.lock` is never removed, and `overlap:
    /// "skip"` refuses every future fire.
    #[tokio::test]
    async fn the_ceiling_gate_does_not_block_updating_an_existing_schedule() {
        let sid = session("ceiling-gate-update");
        let project = tempfile::tempdir().expect("real tempdir");
        let root = super::super::scheduled_run_store_path(project.path(), Some(&sid), None);
        let store = ScheduleStore::new(root, Some(project.path().to_path_buf()));

        let mut record = full_record("already-there", project.path());
        store.write(&record).await.expect("initial write succeeds");

        let _handle = register_capability_ceiling(
            sid.as_str(),
            "org-policy",
            &serde_json::json!({ "allowedTools": ["read"] }),
        )
        .expect("registers");
        assert_eq!(
            schedule_persistence_refusal(Some(&sid), &process_ceiling_resolver),
            Some(SCHEDULE_CEILING_REFUSAL),
            "creating a NEW schedule is refused while the ceiling is active"
        );

        // …and the wind-down of the EXISTING one is not.
        record.active_run_id = None;
        record.paused = true;
        record.updated_at = "2026-09-15T07:00:00.000Z".to_string();
        store
            .write(&record)
            .await
            .expect("an existing schedule must still be writable under a ceiling");
        store
            .append_event(&record, "schedule.paused")
            .await
            .expect("an event must still be appendable under a ceiling");
        store
            .release_active_lock(&record.id)
            .await
            .expect("the lock must still be releasable under a ceiling");

        let reloaded = store.get(&record.id).await.expect("reads back");
        assert_eq!(reloaded.active_run_id, None);
        assert!(reloaded.paused);
    }

    /// §4.3 — a MALFORMED inherited ceiling fails CLOSED.
    ///
    /// Degrading the decode error to "no ceiling" would let a corrupt
    /// `CYRUP_SUBAGENT_CAPABILITY_CEILING_V1` value BUY the right to persist a schedule, which is
    /// strictly worse than the refusal. The env is injected rather than set, because this crate is
    /// `#![forbid(unsafe_code)]` and `std::env::set_var` is `unsafe` under edition 2024.
    #[test]
    fn a_malformed_inherited_ceiling_fails_closed() {
        let malformed = |key: &str| {
            (key == CAPABILITY_CEILING_ENV).then(|| OsString::from("not-base64url-json!!"))
        };
        let resolved = ceiling_resolver_from("no-registration-at-all", &malformed)
            .expect("a malformed inherited ceiling must resolve to a ceiling, not to None");
        assert_eq!(resolved.sources, vec![MALFORMED_CEILING_SOURCE.to_string()]);
        assert_eq!(resolved.allowed_tools, Some(Vec::new()));
        assert_eq!(resolved.allowed_agents, Some(Vec::new()));
        assert!(resolved.deny_extensions);

        assert_eq!(
            schedule_persistence_refusal(Some(&session("no-registration-at-all")), &|key| {
                ceiling_resolver_from(key, &malformed)
            }),
            Some(SCHEDULE_CEILING_REFUSAL),
            "and the refusal text stays the contract string, not a decode diagnostic"
        );
    }

    /// A WELL-FORMED inherited ceiling arriving over the env boundary binds the gate too — the
    /// env half of [`ceiling_resolver_from`] is not decoration.
    #[test]
    fn a_well_formed_inherited_ceiling_refuses_with_no_registration() {
        let encoded = crate::exec::capability_ceiling::encode_capability_ceiling(Some(
            &ResolvedCapabilityCeiling {
                version: CAPABILITY_CEILING_VERSION,
                allowed_tools: Some(vec!["read".to_string()]),
                allowed_agents: None,
                deny_extensions: false,
                sources: vec!["parent".to_string()],
            },
        ))
        .expect("encodes");
        let inherited =
            |key: &str| (key == CAPABILITY_CEILING_ENV).then(|| OsString::from(encoded.clone()));

        assert_eq!(
            schedule_persistence_refusal(Some(&session("inherited-only")), &|key| {
                ceiling_resolver_from(key, &inherited)
            }),
            Some(SCHEDULE_CEILING_REFUSAL),
        );
        assert_eq!(
            schedule_persistence_refusal(Some(&session("inherited-only")), &|key| {
                ceiling_resolver_from(key, &no_env)
            }),
            None,
            "and with nothing inherited and nothing registered there is no gate"
        );
    }
}
