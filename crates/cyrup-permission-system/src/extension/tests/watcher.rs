//! The PARENT forwarding watcher's idempotence, its UI-gated arming and teardown, and the
//! process-wide parent-session anchor.

use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use cyrup_ext::{HostEvent, HostServices, NativeExtension};

use super::support::*;
use crate::extension::{PermissionSystemExtension, guard};

/// PERM-001 (first gap), the publisher half: a PARENT-role extension publishes its live session
/// id into `cyrup-ext-subagents`' process-wide anchor register on `SessionStart` (pi
/// `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`, `pi-subagents/src/extension/
/// index.ts:599` @v0.34.0) and clears it on `SessionShutdown` (`:619`), so the hop-1 detached spawn has
/// an anchor to overlay onto the background runner. Before this, nothing in the workspace ever
/// published the root's id anywhere a spawn could read it, and the detached path resolved an
/// empty target on every hop.
///
/// The anchor register (`cyrup_ext_subagents::background::parent_anchor`) is PROCESS-global and
/// cargo runs this crate's unit tests as parallel threads of one process, so every test that
/// mutates it must hold this lock for its whole body. (This module used to carry a single
/// anchor test for exactly that reason — "one test, not several". A lock is the honest version
/// of that constraint, and lets the CHILD-role gate below be its own test rather than an
/// appendix to the PARENT-role one. Mirrors `parent_anchor.rs`'s own `REGISTER_LOCK`.)
///
/// A `tokio::sync::Mutex`, not a `std` one: every holder below is an `async` test that awaits
/// `on_event` while holding the guard, and a `std::sync::MutexGuard` held across an await point
/// is `clippy::await_holding_lock`. (`parent_anchor.rs`'s `REGISTER_LOCK` can be a `std` mutex
/// because its tests are synchronous.) It also drops the poison handling `std` would force at
/// every call site — a tokio mutex has no poisoning, so a panicking test releases the lock
/// cleanly instead of leaving siblings to recover from a `PoisonError`.
static ANCHOR_REGISTER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A [`HostServices`] whose only override is a fixed `session_id` — the single input
/// `publish_parent_session_anchor` reads.
struct AnchorHost(&'static str);
impl HostServices for AnchorHost {
    fn session_id(&self) -> Option<String> {
        Some(self.0.to_string())
    }
}

#[tokio::test]
async fn parent_role_publishes_and_clears_the_process_parent_session_anchor() {
    let _guard = ANCHOR_REGISTER_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");

    let ext = PermissionSystemExtension::new_forwarding_parent(
        agent_dir.clone(),
        dir.path().to_path_buf(),
    );
    ext.set_host_services(Arc::new(AnchorHost("session-root-perm001")));
    let ctx = event_ctx(dir.path().to_path_buf());

    cyrup_ext_subagents::clear_parent_session_anchor();
    let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx).await;
    assert_eq!(
        cyrup_ext_subagents::background::parent_anchor::published_parent_session_anchor()
            .as_deref(),
        Some("session-root-perm001"),
        "a PARENT-role SessionStart must publish the live session id as the spawn anchor"
    );

    let _ = ext
        .on_event(&HostEvent::SessionShutdown { reason: "exit".to_string(), target_session_file: None }, &ctx)
        .await;
    assert_eq!(
        cyrup_ext_subagents::background::parent_anchor::published_parent_session_anchor(),
        None,
        "SessionShutdown must clear the anchor (pi's `delete process.env[...]`)"
    );
}

/// PERM-001 follow-up — the CHILD half of the publisher gate, and the cross-crate invariant the
/// published-first anchor ladder rests on.
///
/// `cyrup_ext_subagents::background::parent_anchor::resolve_parent_session_anchor` resolves
/// PUBLISHED before INHERITED, emulating pi's single-cell ASSIGNMENT
/// (`process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`, `pi-subagents/src/extension/
/// index.ts:599` @v0.34.0). That ordering is safe for a NESTED orchestrator — one that was
/// itself spawned as a subagent and must keep threading the ROOT's anchor downward rather than
/// substituting its own id — for exactly ONE reason: such a process never publishes, so its
/// register stays empty and the inherited root anchor wins regardless of rung order.
///
/// Upstream enforces that with `if (!process.env[SUBAGENT_CHILD_ENV])` wrapped around the
/// assignment (`index.ts:596-601` @v0.34.0). Cyrup's analog is `install_watcher`, which
/// [`PermissionSystemExtension::new_forwarding_child`] sets to `false` and which
/// `publish_parent_session_anchor` early-returns on — and `permission_extension_for_env` builds
/// exactly that role whenever [`is_subagent_child`] sees a [`SUBAGENT_ENV_HINT_KEYS`] hint.
///
/// NOTHING pinned that gate. If it regressed — a flipped flag, a second publisher, a refactor
/// of `new_forwarding_child` — a nested orchestrator would publish its own id, the register
/// would shadow the inherited root anchor, and a depth-2 grandchild would address its immediate
/// parent's forwarding spool instead of the root's. Every forwarded ask from that subtree would
/// then land on a spool with no watcher on it and fail-closed DENY, silently and with no
/// prompt. This test is the guard for that.
#[tokio::test]
async fn a_subagent_child_never_publishes_or_clears_the_parent_session_anchor() {
    let _guard = ANCHOR_REGISTER_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");

    // A nested orchestrator: itself a subagent child, so `permission_extension_for_env` builds
    // it with `new_forwarding_child` (`install_watcher: false`). It has a perfectly good live
    // session id of its own — the gate, not the absence of an id, is what must stop it.
    let child = PermissionSystemExtension::new_forwarding_child(
        agent_dir.clone(),
        dir.path().to_path_buf(),
    );
    child.set_host_services(Arc::new(AnchorHost("nested-orchestrator-own-id")));
    let ctx = event_ctx(dir.path().to_path_buf());

    cyrup_ext_subagents::clear_parent_session_anchor();
    let _ = child
        .on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx)
        .await;
    assert_eq!(
        cyrup_ext_subagents::background::parent_anchor::published_parent_session_anchor(),
        None,
        "a CHILD-role SessionStart must NOT publish its own id (pi's \
         `if (!process.env[SUBAGENT_CHILD_ENV])` guard, `index.ts:596`) — publishing here is \
         what would make the published-first ladder hand a grandchild the WRONG ancestor"
    );

    // The consequence that makes the reorder safe, asserted directly rather than inferred: with
    // nothing published, the inherited ROOT anchor is what a spawn from this process resolves.
    assert_eq!(
        cyrup_ext_subagents::background::parent_anchor::resolve_parent_session_anchor_from(
            Some("root-session-anchor".to_string())
        ),
        Some("root-session-anchor".to_string()),
        "a nested orchestrator keeps threading the ROOT's anchor downward — this is the case \
         the published-first reorder had to leave untouched, and it holds because the register \
         above is empty"
    );

    // The mirror gate (`SessionShutdown`): a child never published, so it must never CLEAR
    // either — otherwise a child sharing a process with a parent-role session would wipe the
    // anchor out from under it.
    cyrup_ext_subagents::publish_parent_session_anchor("root-session-anchor");
    let _ = child
        .on_event(&HostEvent::SessionShutdown { reason: "exit".to_string(), target_session_file: None }, &ctx)
        .await;
    assert_eq!(
        cyrup_ext_subagents::background::parent_anchor::published_parent_session_anchor()
            .as_deref(),
        Some("root-session-anchor"),
        "a CHILD-role SessionShutdown must leave a published anchor alone (it never published \
         one), or it would clear an anchor that is not its to clear"
    );

    cyrup_ext_subagents::clear_parent_session_anchor();
}

// ============================================================================================
// PERM-005 — the forwarding watcher must be (re)armed on EVERY hook pi arms it on, idempotently,
// and torn down when the context stops qualifying.
//
// Upstream calls `startForwardedPermissionPolling(ctx)` from four places —
// `refreshSessionRuntimeState` (`index.ts:2084`, reached from `session_start`),
// `before_agent_start` (`:2137`), `input` (`:2194`) and `tool_call` (`:2210`) — and calls
// `stopForwardedPermissionPolling()` from `session_shutdown` (`:2131`) AND from the
// disqualified branch of the start function itself (`:1985`).
//
// Cyrup had exactly ONE caller (`SessionStart`) and a guard that returned without stopping.
// ============================================================================================

/// A [`HostServices`] with a fixed session id, standing in for a live parent backend.
struct WatcherHost(String);
impl HostServices for WatcherHost {
    fn session_id(&self) -> Option<String> {
        Some(self.0.clone())
    }
}

/// Builds a PARENT-role extension AND takes [`ANCHOR_REGISTER_LOCK`], returning the guard the
/// caller must hold for the rest of the test.
///
/// The guard is bundled rather than left to each caller because the coupling is INVISIBLE at
/// the call site: none of these PERM-005 watcher tests mentions the parent-session anchor, but
/// every one of them fires a PARENT-role `SessionStart`, and that hook calls
/// `publish_parent_session_anchor` as a SIDE EFFECT — writing the process-global register that
/// `parent_role_publishes_and_clears_the_process_parent_session_anchor` and
/// `a_subagent_child_never_publishes_or_clears_the_parent_session_anchor` assert on. Four
/// unsynchronized writers against two asserting readers in one test binary is a live race: it
/// was observed failing the child-gate assertion with `Some("perm005-detach")` — this helper's
/// own session id — leaking in from `a_detaching_ui_tears_the_forwarding_watcher_down`.
///
/// Returning the guard makes that safety automatic for any FUTURE watcher test too, instead of
/// depending on its author noticing an anchor coupling nothing in the test text mentions.
async fn parent_ext(
    dir: &Path,
    session: &str,
) -> (tokio::sync::MutexGuard<'static, ()>, PermissionSystemExtension) {
    let guard = ANCHOR_REGISTER_LOCK.lock().await;
    let agent_dir = dir.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let ext = PermissionSystemExtension::new_forwarding_parent(agent_dir, dir.to_path_buf());
    ext.set_host_services(Arc::new(WatcherHost(session.to_string())));
    (guard, ext)
}

/// The BASELINE `live_watcher_task_count` subtracts. It is a hand-maintained constant naming
/// the structural `config` holders (`self.config`, `self.logger`, `self.controller`), and it
/// has already gone stale once: PERM-007 added the `ConfigController` as a third holder, and
/// every watcher-count assertion silently started reading one watcher too many.
///
/// Pinned here, on an extension with NO watcher armed, so a future holder trips this test —
/// which names the cause — instead of only the PERM-005 tests, which would blame a watcher leak
/// that never happened.
#[test]
fn a_fresh_extension_holds_no_watcher_config_handles() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let ext =
        PermissionSystemExtension::new_forwarding_parent(agent_dir, dir.path().to_path_buf());
    assert_eq!(
        ext.live_watcher_task_count(),
        0,
        "no hook has run, so no watcher exists; a non-zero count means a new structural holder \
         of the shared `config` handle was added without updating NON_WATCHER_CONFIG_HOLDERS"
    );
}

/// PERM-005, the crux: the three per-turn hooks fire on EVERY turn, so a non-idempotent start
/// would spawn one watcher per turn. N calls must yield exactly ONE live watcher task.
#[tokio::test]
async fn repeated_hooks_yield_exactly_one_forwarding_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let (_anchor_guard, ext) = parent_ext(dir.path(), "perm005-idem").await;
    let ctx = ui_ctx(dir.path());

    let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".into(), previous_session_file: None }, &ctx).await;
    assert!(ext.has_live_forwarding_watcher(), "SessionStart must arm the watcher");

    // Ten more turns' worth of hooks — the exact re-entry pi performs.
    for _ in 0..10 {
        let _ = ext
            .on_event(
                &HostEvent::BeforeAgentStart {
                    prompt: String::new(),
                    images: json!(null),
                    system_prompt: String::new(),
                    options: json!(null),
                    injected: Vec::new(),
                },
                &ctx,
            )
            .await;
        let _ = ext
            .on_event(&HostEvent::Input {
                text: "hello".into(),
                images: Vec::new(),
                source: cyrup_ext::InputEventSource::Interactive,
                streaming_behavior: None,
            }, &ctx)
            .await;
        let _ = ext
            .on_event(
                &HostEvent::ToolCall {
                    call_id: cyrup_core::ToolCallId::from("c1"),
                    name: "read".into(),
                    input: json!({}),
                },
                &ctx,
            )
            .await;
    }

    assert!(ext.has_live_forwarding_watcher(), "the watcher must still be live");
    assert_eq!(
        ext.live_watcher_task_count(),
        1,
        "31 hook re-entries must leave EXACTLY one watcher task — a non-idempotent start would \
         have leaked one per turn (pi `index.ts:1996-2000` keeps the existing watcher)"
    );

    ext.stop_forwarding_watcher();
}

/// PERM-005 failure mode (2): a UI that attaches AFTER `SessionStart` must still get a watcher.
/// Pre-fix, `SessionStart` was the only caller, so a session that was headless at start never
/// armed one for its whole life and every forwarded child ask sat in the spool until it failed
/// closed.
#[tokio::test]
async fn a_later_hook_arms_the_watcher_a_headless_session_start_could_not() {
    let dir = tempfile::tempdir().unwrap();
    let (_anchor_guard, ext) = parent_ext(dir.path(), "perm005-late-ui").await;

    let _ = ext
        .on_event(
            &HostEvent::SessionStart { reason: "startup".into(), previous_session_file: None },
            &headless_ctx(dir.path()),
        )
        .await;
    assert!(
        !ext.has_live_forwarding_watcher(),
        "a headless SessionStart must not arm a watcher (pi `:1726`)"
    );

    // The UI attaches; the very next turn's `tool_call` re-enters the start function.
    let _ = ext
        .on_event(
            &HostEvent::ToolCall {
                call_id: cyrup_core::ToolCallId::from("c1"),
                name: "read".into(),
                input: json!({}),
            },
            &ui_ctx(dir.path()),
        )
        .await;
    assert!(
        ext.has_live_forwarding_watcher(),
        "pi re-enters `startForwardedPermissionPolling` from `tool_call` (`index.ts:2210`), so \
         a late-attaching UI must arm the watcher"
    );

    ext.stop_forwarding_watcher();
}

/// PERM-005 failure mode (3): a UI that DETACHES mid-session must tear the watcher down. pi's
/// disqualified branch calls `stopForwardedPermissionPolling()` before returning
/// (`index.ts:1984-1987`); cyrup's guard used to `return` and leave the task prompting into a
/// backend with no human behind it.
#[tokio::test]
async fn a_detaching_ui_tears_the_forwarding_watcher_down() {
    let dir = tempfile::tempdir().unwrap();
    let (_anchor_guard, ext) = parent_ext(dir.path(), "perm005-detach").await;

    let _ = ext
        .on_event(&HostEvent::SessionStart { reason: "startup".into(), previous_session_file: None }, &ui_ctx(dir.path()))
        .await;
    assert!(ext.has_live_forwarding_watcher(), "SessionStart with a UI arms the watcher");

    let _ = ext
        .on_event(
            &HostEvent::Input {
                text: "hello".into(),
                images: Vec::new(),
                source: cyrup_ext::InputEventSource::Interactive,
                streaming_behavior: None,
            },
            &headless_ctx(dir.path()),
        )
        .await;
    assert!(
        !ext.has_live_forwarding_watcher(),
        "a hook on a no-UI context must STOP the watcher, not merely decline to start one"
    );
}

/// PERM-005 failure mode (4): the watcher must observe a mid-session `config.json` change.
/// It now shares the extension's `config` mutex instead of capturing a snapshot by value, so
/// `refresh_config_and_manager` (pi `refreshExtensionConfig`, `index.ts:1600-1608`) reaches the
/// running task. Asserted structurally — the running watcher and the extension must be looking
/// at the SAME `ExtensionConfig`.
#[tokio::test]
async fn the_running_watcher_shares_the_extensions_live_config() {
    let dir = tempfile::tempdir().unwrap();
    let (_anchor_guard, ext) = parent_ext(dir.path(), "perm005-config").await;
    let ctx = ui_ctx(dir.path());

    let _ = ext.on_event(&HostEvent::SessionStart { reason: "startup".into(), previous_session_file: None }, &ctx).await;
    assert_eq!(ext.live_watcher_task_count(), 1, "one watcher, holding the shared handle");

    // The watcher's handle IS the extension's handle: a write here is visible to the task.
    assert!(!guard(&ext.config).yolo_mode);
    guard(&ext.config).yolo_mode = true;
    assert!(
        guard(&ext.config).yolo_mode,
        "the config the watcher reads per poll iteration is the one the extension mutates"
    );

    ext.stop_forwarding_watcher();
}

// ==================================================================== PERM-011 (both halves)
