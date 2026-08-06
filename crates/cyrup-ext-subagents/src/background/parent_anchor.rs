//! The process-wide parent-session anchor register (R-SA-P1, PERM-001) — cyrup's `unsafe`-free
//! stand-in for pi's `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`.
//!
//! # What this ports, and why it has to exist at all
//!
//! Upstream `pi-subagents` publishes the launching session's own id into its OWN process
//! environment at `SessionStart` (`src/extension/index.ts:555`,
//! `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`) and deletes it again at session
//! shutdown (`:584`). Every subsequent spawn — foreground, background, detached, at any hop —
//! therefore inherits the anchor for free, because `pi-args.ts:416` resolves it as
//! `input.parentSessionId ?? process.env[SUBAGENT_PARENT_SESSION_ENV] ?? ""` and a detached child
//! inherits the parent's environment block whether or not the spawn site adds an overlay entry.
//! `pi-permission-system` then reads that anchor (as its own
//! `SUBAGENT_PARENT_SESSION_ENV_KEY = "PI_AGENT_ROUTER_PARENT_SESSION_ID"`,
//! `permission-forwarding.ts:10,132`) to address the parent's ask-forwarding inbox.
//!
//! cyrup cannot do the `process.env` half: this crate is `#![forbid(unsafe_code)]` and
//! `std::env::set_var` is `unsafe` as of the 2024 edition. `SubagentExecutor` consequently keeps
//! the captured root anchor in a private `Mutex` field (`extension.rs`
//! `capture_parent_session_anchor`/`clear_parent_session_anchor`, themselves a faithful port of
//! index.ts:555/584) and threads it EXPLICITLY through
//! [`crate::exec::RunOptions::parent_session_id`] on the FOREGROUND path.
//!
//! That explicit thread is the only channel that existed, and it stops at the foreground path.
//! The BACKGROUND path is two OS hops (orchestrator → detached `__subagent-runner` → the actual
//! subagent child), and neither hop carried the anchor: the hop-1 spawn
//! ([`super::spawn_detached`]) added no env overlay at all, so the hop-2 runner process's own
//! environment had no `CYRUP_SUBAGENT_PARENT_SESSION` to inherit, so the hop-3 spawn's
//! "explicit → inherited env → empty" ladder in `exec::build_attempt_spawn_plan` fell through to
//! EMPTY. A background subagent that hit an `ask` therefore addressed a null target and
//! `cyrup-permission-system`'s `forwarding::wait_for_forwarded_approval` took its fail-closed
//! null-target deny branch (pi `index.ts:1267-1272`) — the operator saw an unexplained tool denial
//! and was never prompted (PERM-001).
//!
//! This module restores pi's process-global rung in a memory-safe way: a plain `Mutex`-backed
//! register with exactly pi's set/clear lifecycle, published by whichever component in this
//! process knows the live session id and consulted by [`super::spawn_detached`] when it builds the
//! hop-1 env overlay.
//!
//! # Who publishes
//!
//! The register's sole CONSUMER is `cyrup-permission-system` (the anchor exists only so a child's
//! forwarded ask can address its parent's spool — nothing else in cyrup reads
//! [`crate::PARENT_SESSION_ENV_VAR`]), and that crate is also the one that publishes here, from
//! its PARENT-role `SessionStart`/`SessionShutdown` handlers, mirroring pi's index.ts:555/584
//! placement one crate over. Publishing is strictly a PARENT-role act: a subagent child must never
//! overwrite the anchor with its own id, or a depth-2 grandchild would address its immediate
//! parent instead of continuing to thread the root's anchor (the direct-parent depth-1 semantics
//! `exec::PARENT_SESSION_ENV_VAR` documents).
//!
//! When nothing publishes (no permission system installed ⇒ no gate ⇒ no asks ⇒ no forwarding),
//! the register stays empty and [`resolve_parent_session_anchor`] degrades to exactly the previous
//! env-only behavior.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::exec::PARENT_SESSION_ENV_VAR;

/// The process-wide anchor slot — pi's `process.env[SUBAGENT_PARENT_SESSION_ENV]` cell
/// (`pi-subagents/src/extension/index.ts:555`), held as a `Mutex` because this crate forbids the
/// `unsafe` `std::env::set_var` a real env write would require. A plain `Mutex` (not a `OnceLock`)
/// for the same reason `SubagentExecutor::root_parent_session` is one: pi's slot is CLEARABLE at
/// session shutdown (`:584`'s `delete`), which a write-once cell could not support.
static ROOT_PARENT_SESSION_ANCHOR: Mutex<Option<String>> = Mutex::new(None);

/// Publish this process's live session id as the parent-session anchor future spawns address
/// (pi `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`, `extension/index.ts:555`).
///
/// A blank id clears the slot rather than storing an empty string, so a headless / unpersisted
/// session never installs an anchor that would later resolve to a null forwarding target.
///
/// Call ONLY from a PARENT-role (root orchestrator) session start — see the module docs on why a
/// subagent child must not publish its own id.
pub fn publish_parent_session_anchor(session_id: &str) {
    let trimmed = session_id.trim();
    let mut slot = ROOT_PARENT_SESSION_ANCHOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
}

/// Clear the published anchor (pi `delete process.env[SUBAGENT_PARENT_SESSION_ENV]`,
/// `extension/index.ts:584`), so a stale id from a session that just ended never leaks into a
/// subsequently-started session on this same long-lived process.
pub fn clear_parent_session_anchor() {
    *ROOT_PARENT_SESSION_ANCHOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// The currently published anchor, if any — the register half of
/// [`resolve_parent_session_anchor`]'s ladder, exposed for assertions and diagnostics.
#[must_use]
pub fn published_parent_session_anchor() -> Option<String> {
    ROOT_PARENT_SESSION_ANCHOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Resolve the anchor this process should hand to a child it is about to spawn: INHERITED (this
/// process's own `CYRUP_SUBAGENT_PARENT_SESSION`) → PUBLISHED (the register above) → none.
///
/// The inherited value wins deliberately, matching `exec::build_attempt_spawn_plan`'s own ladder:
/// a process that is ITSELF a subagent child keeps threading the root's anchor downward rather
/// than substituting anything of its own. Only a non-empty value is ever returned, so a caller can
/// treat `Some` as "safe to write into a child's environment".
#[must_use]
pub fn resolve_parent_session_anchor() -> Option<String> {
    std::env::var(PARENT_SESSION_ENV_VAR)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(published_parent_session_anchor)
}

/// The env overlay the hop-1 detached spawn ([`super::spawn_detached::spawn_detached_runner`])
/// applies to the `__subagent-runner` process it launches: `{CYRUP_SUBAGENT_PARENT_SESSION:
/// <anchor>}` when one resolves, otherwise EMPTY.
///
/// This is the whole PERM-001 repair on the writer side. pi gets this for free from environment
/// inheritance because its anchor lives in `process.env`; cyrup has to write the one entry
/// explicitly at the hop-1 boundary so the detached runner's own environment carries it and
/// `exec::build_attempt_spawn_plan`'s inherited rung then feeds every hop-3 child exactly as it
/// does on the foreground path.
///
/// An empty map is returned (rather than an entry with an empty value) when no anchor resolves:
/// a spawn env is an OVERLAY over the inherited environment, and writing `""` would MASK an anchor
/// the runner would otherwise have inherited.
#[must_use]
pub fn detached_runner_env_overlay() -> BTreeMap<String, String> {
    let mut overlay = BTreeMap::new();
    if let Some(anchor) = resolve_parent_session_anchor() {
        overlay.insert(PARENT_SESSION_ENV_VAR.to_string(), anchor);
    }
    overlay
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// One test, not several: the register is PROCESS-global, and cargo runs a crate's unit tests
    /// as parallel threads of one process, so two tests mutating it would race each other.
    ///
    /// Covers (a) the pi-index.ts:555/584 set/clear round trip, (b) blank publishes CLEARING rather
    /// than storing `""` (a stored empty string would resolve as a forwarding target and then be
    /// rejected as a null target downstream by `forwarding::normalize_session_id` — strictly worse
    /// than no anchor), and (c) the PERM-001 regression itself: with a published anchor and no
    /// inherited env value — the exact state of a ROOT orchestrator, since nothing in this
    /// workspace ever sets `CYRUP_SUBAGENT_PARENT_SESSION` for a process's OWN env — the hop-1
    /// detached spawn overlay carries the anchor. Before the fix this map did not exist and the
    /// hop-1 spawn added no env at all, so the detached runner (and therefore every child it
    /// spawned) had no anchor and every forwarded ask fail-closed denied.
    ///
    /// The overlay half is skipped when the ambient env already carries an anchor (this crate
    /// forbids `unsafe`, so a test cannot clear one); the inherited-wins direction is pinned by
    /// `tests/background_spawn_detached_integration.rs`.
    #[test]
    fn register_round_trips_and_feeds_the_detached_runner_overlay() {
        publish_parent_session_anchor("session-root-1");
        assert_eq!(published_parent_session_anchor().as_deref(), Some("session-root-1"));

        publish_parent_session_anchor("   ");
        assert_eq!(published_parent_session_anchor(), None);

        publish_parent_session_anchor("  session-root-2  ");
        assert_eq!(published_parent_session_anchor().as_deref(), Some("session-root-2"));

        if std::env::var(PARENT_SESSION_ENV_VAR).is_ok_and(|v| !v.trim().is_empty()) {
            clear_parent_session_anchor();
            return;
        }

        let overlay = detached_runner_env_overlay();
        assert_eq!(
            overlay.get(PARENT_SESSION_ENV_VAR).map(String::as_str),
            Some("session-root-2"),
            "the hop-1 detached spawn must carry the published root anchor into the runner's env"
        );

        clear_parent_session_anchor();
        assert_eq!(published_parent_session_anchor(), None);
        assert!(
            detached_runner_env_overlay().is_empty(),
            "no anchor ⇒ EMPTY overlay, never an empty-valued entry that would mask an inherited one"
        );
    }
}
