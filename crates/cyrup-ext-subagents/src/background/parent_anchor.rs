//! The process-wide parent-session anchor register (R-SA-P1, PERM-001) — cyrup's `unsafe`-free
//! stand-in for pi's `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`.
//!
//! # What this ports, and why it has to exist at all
//!
//! Upstream `pi-subagents` publishes the launching session's own id into its OWN process
//! environment at `SessionStart` (`src/extension/index.ts:713-718` @v0.43.0 — the assignment
//! `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId` is `:599`, guarded by
//! `if (!process.env[SUBAGENT_CHILD_ENV])`) and deletes it again at session shutdown (`:619`).
//! Every subsequent spawn — foreground, background, detached, at any hop — therefore inherits the
//! anchor for free, because `runs/shared/pi-args.ts:257` @v0.34.0 resolves it as
//! `input.parentSessionId ?? process.env[SUBAGENT_PARENT_SESSION_ENV] ?? ""` and a detached child
//! inherits the parent's environment block whether or not the spawn site adds an overlay entry.
//! `pi-permission-system` then reads that anchor (as its own
//! `SUBAGENT_PARENT_SESSION_ENV_KEY = "PI_AGENT_ROUTER_PARENT_SESSION_ID"`,
//! `permission-forwarding.ts:9,144` @v0.7.1) to address the parent's ask-forwarding inbox.
//!
//! (Those citations previously named `extension/index.ts:555`/`:584`, `runs/shared/pi-args.ts:416` and
//! `permission-forwarding.ts:10,132` — the lines those statements occupy at each upstream's
//! **HEAD**, not at the versions this workspace ports. `cyrup-ext-subagents` cites
//! `pi-subagents` v0.34.0 throughout, `cyrup-permission-system` cites `pi-permission-system`
//! v0.7.1; the numbers above are the v0.34.0/v0.7.1 lines, verified against
//! `git show v0.34.0:src/extension/index.ts`.)
//!
//! cyrup cannot do the `process.env` half: this crate is `#![forbid(unsafe_code)]` and
//! `std::env::set_var` is `unsafe` as of the 2024 edition. `SubagentExecutor` consequently keeps
//! the captured root anchor in a private `Mutex` field (`extension.rs`
//! `capture_parent_session_anchor`/`clear_parent_session_anchor`, themselves a faithful port of
//! extension/index.ts:599/619) and threads it EXPLICITLY through
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
//! its PARENT-role `SessionStart`/`SessionShutdown` handlers, mirroring pi's extension/index.ts:599/619
//! placement one crate over. Publishing is strictly a PARENT-role act — the exact analog of pi's
//! own `if (!process.env[SUBAGENT_CHILD_ENV])` guard at `extension/index.ts:596`: a subagent child must never
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
/// (`pi-subagents/src/extension/index.ts:716`), held as a `Mutex` because this crate forbids the
/// `unsafe` `std::env::set_var` a real env write would require. A plain `Mutex` (not a `OnceLock`)
/// for the same reason `SubagentExecutor::root_parent_session` is one: pi's slot is CLEARABLE at
/// session shutdown (`:619`'s `delete`), which a write-once cell could not support.
static ROOT_PARENT_SESSION_ANCHOR: Mutex<Option<String>> = Mutex::new(None);

/// Publish this process's live session id as the parent-session anchor future spawns address
/// (pi `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`, `extension/index.ts:716`).
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
/// `extension/index.ts:619`), so a stale id from a session that just ended never leaks into a
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

/// Resolve the anchor this process should hand to a child it is about to spawn: PUBLISHED (the
/// register above) → INHERITED (this process's own `CYRUP_SUBAGENT_PARENT_SESSION`) → none.
///
/// # Why PUBLISHED wins (PERM-001 follow-up)
///
/// pi has exactly ONE cell for this, `process.env[SUBAGENT_PARENT_SESSION_ENV]`, and publishing is
/// an **assignment** (`extension/index.ts:599`): a session that publishes CLOBBERS whatever the process
/// inherited, and a spawn afterwards can only ever read the published value. cyrup cannot write
/// its own environment, so it emulates that one cell with TWO — the immutable inherited env plus
/// [`ROOT_PARENT_SESSION_ANCHOR`] — and the only faithful emulation of an assignment is for the
/// register to SHADOW the inherited value.
///
/// This ladder used to read INHERITED → PUBLISHED, justified in its own doc comment as "matching
/// `exec::build_attempt_spawn_plan`'s own ladder". It does not match it: that ladder is EXPLICIT
/// (`opts.parent_session_id`, sourced from `SubagentExecutor::root_parent_session` — i.e. the
/// captured/published anchor) → INHERITED (`exec/mod.rs`, pi `input.parentSessionId ??
/// process.env[…]`, `runs/shared/pi-args.ts:257`). Published-first is what makes the two agree, and what makes
/// the foreground and background paths agree with each other.
///
/// "But a nested orchestrator must keep threading the ROOT's anchor, not substitute its own" is
/// still true, and is still enforced — by the PUBLISHER, exactly where pi enforces it: a subagent
/// child never publishes. Upstream that is the `if (!process.env[SUBAGENT_CHILD_ENV])` guard at
/// `extension/index.ts:596`; here it is `PermissionSystemExtension::publish_parent_session_anchor`'s
/// `install_watcher` gate, which is the SOLE writer of this register (a process carrying any of
/// `cyrup-permission-system`'s `SUBAGENT_ENV_HINT_KEYS` is built as `new_forwarding_child`, whose
/// `install_watcher` is `false` — the direct analog of pi's `SUBAGENT_CHILD_ENV` test). The hop-2
/// `__subagent-runner` process never publishes either, for a stronger reason: `main.rs`
/// pre-dispatches `__subagent-runner` before any session or extension is built, so no extension of
/// any kind exists in it — its [`resolve_parent_session_anchor`] call
/// (`background::runner_main`) reads an empty register and returns the inherited root anchor,
/// under either ordering.
///
/// (`SubagentsExtension`'s own `SessionStart` arm — likewise unreachable for a
/// `RegistrationMode::ChildSafe` child, which does not subscribe to it — calls
/// `SubagentExecutor::capture_parent_session_anchor`, a DIFFERENT cell: the executor's private
/// `root_parent_session` field feeding `RunOptions::parent_session_id` on the foreground path. It
/// does not write this register.)
///
/// So with no publisher in a child process the register is empty and the inherited root anchor is
/// returned either way — reordering cannot regress the nesting case. What it fixes is the case the
/// old order got wrong, and which pi's assignment makes impossible: a PARENT-role session in a
/// process that already carried `CYRUP_SUBAGENT_PARENT_SESSION` in its environment WITHOUT any
/// subagent hint (an SDK embedder, a test harness, or any launcher/CI that exported the variable —
/// no hint means `permission_extension_for_env` builds the PARENT role and publishes). Such a
/// session handed its detached runner a STALE ANCESTOR's id instead of its own live one, so every
/// forwarded ask from that background subtree addressed a spool with no watcher on it and
/// fail-closed denied.
///
/// Only a non-empty value is ever returned, so a caller can treat `Some` as "safe to write into a
/// child's environment".
#[must_use]
pub fn resolve_parent_session_anchor() -> Option<String> {
    resolve_parent_session_anchor_from(std::env::var(PARENT_SESSION_ENV_VAR).ok())
}

/// The injectable core of [`resolve_parent_session_anchor`], parameterized over the INHERITED env
/// value so the ladder is directly testable — this crate is `#![forbid(unsafe_code)]` and
/// `std::env::set_var` is `unsafe` as of the 2024 edition, so a test cannot install an inherited
/// value to assert against. Mirrors the same injectable-core convention
/// `spawn::resolve_spawn_command_from` / `spawn::depth::resolve_effective_depth_from` /
/// `background::spawn_detached::spawn_detached_runner_with_command` already use.
///
/// Both rungs are trimmed and empty-filtered before they can win, so neither an inherited `""`
/// (which a spawn overlay could legitimately carry) nor a whitespace-only publish is ever returned.
#[must_use]
pub fn resolve_parent_session_anchor_from(inherited: Option<String>) -> Option<String> {
    published_parent_session_anchor().or_else(|| {
        inherited
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
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

    /// The register is PROCESS-global and cargo runs a crate's unit tests as parallel threads of
    /// ONE process, so every test in this module that mutates it must hold this lock for its whole
    /// body. (The module used to carry a single test for exactly this reason; a lock is the honest
    /// version of that constraint and lets the ladder-ordering regression below be its own test
    /// rather than an appendix to an unrelated one.)
    static REGISTER_LOCK: Mutex<()> = Mutex::new(());

    /// Covers (a) the pi-index.ts:599/619 set/clear round trip, (b) blank publishes CLEARING rather
    /// than storing `""` (a stored empty string would resolve as a forwarding target and then be
    /// rejected as a null target downstream by `forwarding::normalize_session_id` — strictly worse
    /// than no anchor), and (c) the PERM-001 regression itself: with a published anchor and no
    /// inherited env value — the exact state of a ROOT orchestrator, since nothing in this
    /// workspace ever sets `CYRUP_SUBAGENT_PARENT_SESSION` for a process's OWN env — the hop-1
    /// detached spawn overlay carries the anchor. Before the fix this map did not exist and the
    /// hop-1 spawn added no env at all, so the detached runner (and therefore every child it
    /// spawned) had no anchor and every forwarded ask fail-closed denied.
    ///
    /// The overlay half no longer needs the ambient-anchor skip the previous version carried: with
    /// PUBLISHED-first resolution a published anchor wins over any inherited one, which is exactly
    /// the assertion below. The empty-overlay half still skips, since an ambient anchor legitimately
    /// produces a non-empty overlay once nothing is published.
    #[test]
    fn register_round_trips_and_feeds_the_detached_runner_overlay() {
        let _guard = REGISTER_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        publish_parent_session_anchor("session-root-1");
        assert_eq!(published_parent_session_anchor().as_deref(), Some("session-root-1"));

        publish_parent_session_anchor("   ");
        assert_eq!(published_parent_session_anchor(), None);

        publish_parent_session_anchor("  session-root-2  ");
        assert_eq!(published_parent_session_anchor().as_deref(), Some("session-root-2"));

        let overlay = detached_runner_env_overlay();
        assert_eq!(
            overlay.get(PARENT_SESSION_ENV_VAR).map(String::as_str),
            Some("session-root-2"),
            "the hop-1 detached spawn must carry the published root anchor into the runner's env"
        );

        clear_parent_session_anchor();
        assert_eq!(published_parent_session_anchor(), None);
        if std::env::var(PARENT_SESSION_ENV_VAR).is_ok_and(|v| !v.trim().is_empty()) {
            return; // an ambient anchor legitimately fills the overlay once nothing is published
        }
        assert!(
            detached_runner_env_overlay().is_empty(),
            "no anchor ⇒ EMPTY overlay, never an empty-valued entry that would mask an inherited one"
        );
    }

    /// PERM-001 follow-up — the LADDER ORDERING regression, driven through the injectable core so
    /// the INHERITED rung can be supplied without `unsafe { std::env::set_var }`.
    ///
    /// pi's anchor lives in ONE cell and publishing is an ASSIGNMENT (`extension/index.ts:599`), so a session
    /// that publishes clobbers whatever it inherited and every subsequent spawn reads the published
    /// value. This module emulates that cell with two, and until this fix the reader consulted them
    /// INHERITED-first — which inverts the assignment: a stale ancestor id, whether inherited from a
    /// launcher's environment or left over in a long-lived embedder process, outranked the live
    /// session's own published id. Every background child spawned from such a session then addressed
    /// a forwarding spool nobody was watching, and its asks fail-closed denied with no prompt.
    ///
    /// The first assertion below is the one that FAILS against the pre-fix ladder. The rest pin the
    /// rungs that must NOT change: with nothing published, the inherited value still wins (this is
    /// the nested-orchestrator case — a subagent child never publishes, in cyrup as in pi, so it
    /// keeps threading the root's anchor downward), and an inherited empty/blank value is never
    /// returned as an anchor.
    #[test]
    fn published_anchor_shadows_an_inherited_one_like_pis_assignment() {
        let _guard = REGISTER_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        // A PARENT-role session that ALSO inherited an anchor: pi's assignment makes its own id win.
        publish_parent_session_anchor("live-root-session");
        assert_eq!(
            resolve_parent_session_anchor_from(Some("stale-ancestor-session".to_string())),
            Some("live-root-session".to_string()),
            "publishing is an ASSIGNMENT upstream (`index.ts:599`): the live session's own id must \
             shadow whatever this process inherited, or its background subtree forwards asks to a \
             spool with no watcher on it"
        );

        // Nothing published (the nested-orchestrator / subagent-child case, which never publishes):
        // the inherited root anchor is threaded downward unchanged.
        clear_parent_session_anchor();
        assert_eq!(
            resolve_parent_session_anchor_from(Some("  root-session  ".to_string())),
            Some("root-session".to_string()),
            "with no publisher the inherited anchor still wins, trimmed — a depth-2 grandchild must \
             keep addressing the ROOT, not its immediate parent"
        );

        // Neither rung ever yields an empty anchor.
        assert_eq!(resolve_parent_session_anchor_from(Some("   ".to_string())), None);
        assert_eq!(resolve_parent_session_anchor_from(None), None);
        publish_parent_session_anchor("   ");
        assert_eq!(resolve_parent_session_anchor_from(Some("inherited".to_string())), Some("inherited".to_string()),
            "a blank publish CLEARS the register rather than shadowing with an empty string");

        clear_parent_session_anchor();
    }
}
