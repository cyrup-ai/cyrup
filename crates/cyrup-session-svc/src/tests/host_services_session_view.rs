//! The `session` interface's READ-ONLY view (pi `ctx.sessionManager`) plus `scopedModels` — the
//! entries/branch/tree reads answered from the LIVE `SessionManager`, and the honest default-host
//! answers when nothing is attached.
//!
//! One of the five files the inline `mod tests` in `host_services.rs` became when that file was
//! split into `src/host_services/`; this is the section its `session read-only view
//! (ctx.sessionManager)` banner opened. Shares [`super::host_services_core::svc_with`] with its
//! siblings.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_ext::host::HostServices;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use cyrup_session::manager::SessionManager;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use super::host_services_core::svc_with;

/// The READ half of the `session` interface must answer from the LIVE tree.
///
/// pi binds the real `ReadonlySessionManager` onto the BASE extension context
/// (`get sessionManager() { runner.assertActive(); return runner.sessionManager }`,
/// `core/extensions/runner.ts:694-697`, typed at `core/extensions/types.ts:317`), so
/// `getEntries()`/`getBranch()`/`getTree()` are truthful in every mode upstream. cyrup's ONLY
/// production backend overrode none of them, so every guest read `[]`/`[]`/`null` forever —
/// indistinguishable from a genuinely fresh session, with no error and no log line, exactly the
/// shape the EXT-005 ctx-state postmortem describes.
///
/// RED before the fix on all three attached assertions (they returned the trait defaults
/// `json!([])`/`json!([])`/`Value::Null` regardless of the attached manager); the UNATTACHED
/// assertions pass either way and are here to pin that the honest default-host answer survives.
#[test]
fn session_read_view_answers_from_the_live_tree() {
    use cyrup_session::manager::NewSessionOpts;

    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);

    // No manager attached (the default host / a by-value session): the trait defaults ARE the
    // honest answer — there is no session to report on.
    assert_eq!(svc.entries(), json!([]), "unattached ⇒ pi's empty read");
    assert_eq!(svc.branch(), json!([]), "unattached ⇒ pi's empty read");
    assert_eq!(svc.tree(), Value::Null, "unattached ⇒ the trait's null tree");

    let mut mgr = SessionManager::in_memory(&std::env::temp_dir(), NewSessionOpts::default())
        .expect("an in-memory session tree");
    let root = mgr.append_custom_entry("note", Some(json!({"n": 1}))).expect("append root");
    let leaf = mgr.append_custom_entry("note", Some(json!({"n": 2}))).expect("append leaf");
    let label = mgr.append_label(&root, Some("checkpoint")).expect("label the root");
    let (root, leaf, label) = (root.to_string(), leaf.to_string(), label.to_string());
    svc.attach_session(Arc::new(AsyncMutex::new(mgr)));

    // `entries` — pi `SessionManager.getEntries()`: every entry except the header. The label is
    // itself an appended entry, so three rows, and the two notes are among them BY ID.
    let entries = svc.entries();
    let ids: Vec<&str> =
        entries.as_array().expect("an array").iter().filter_map(|e| e["id"].as_str()).collect();
    assert!(ids.contains(&root.as_str()), "the live tree's entries reached the guest: {ids:?}");
    assert!(ids.contains(&leaf.as_str()), "the live tree's entries reached the guest: {ids:?}");

    // `branch` — pi `SessionManager.getBranch()`: walk parent-ward from the CURRENT leaf, then
    // reverse. Its doc is explicit that the walk "Includes all entry types (messages,
    // compaction, model changes, etc.)", and `appendLabelChange` builds its `LabelEntry` with
    // `parentId: this.leafId` and then `_appendEntry`s it — so labelling APPENDS to the path
    // rather than annotating off it, and the label entry is the branch head here.
    // `SessionManager::append_label` is the same mechanism (`push_entry` of a
    // `KnownEntry::Label`), so the path is asserted exactly rather than by containment.
    let branch = svc.branch();
    let branch_ids: Vec<&str> =
        branch.as_array().expect("an array").iter().filter_map(|e| e["id"].as_str()).collect();
    assert_eq!(
        branch_ids,
        vec![root.as_str(), leaf.as_str(), label.as_str()],
        "the branch is the whole root→leaf path, in order: {branch_ids:?}"
    );

    // `tree` — pi `SessionManager.getTree()` → `SessionTreeNode[]`, nested, carrying `label`
    // AND SEAM-060's `labelTimestamp` because it shares `tree_node_to_json` with the RPC
    // `get_tree` reply rather than re-deriving the node shape.
    let tree = svc.tree();
    let roots = tree.as_array().expect("an array of roots");
    assert_eq!(roots.len(), 1, "a well-formed session has exactly one root: {tree}");
    assert_eq!(roots[0]["entry"]["id"], json!(root));
    assert_eq!(roots[0]["label"], json!("checkpoint"), "labels survive the serialization");
    assert!(
        roots[0]["labelTimestamp"].is_string(),
        "SEAM-060's labelTimestamp must not be dropped on this side either: {tree}"
    );
    let kids = roots[0]["children"].as_array().expect("children");
    assert_eq!(kids.len(), 1, "the leaf hangs off the root: {tree}");
    assert_eq!(kids[0]["entry"]["id"], json!(leaf));
}

/// EXT-045 — `scoped_models` must report the session's REAL scoped set, in pi's
/// `ScopedModel` shape (`{model, thinkingLevel?}`, `core/model-resolver.ts:63-67`).
///
/// pi exposes it on the base context (`getScopedModels()`, `core/extensions/runner.ts:706-709`;
/// `getScopedModels: () => this._scopedModels`, `core/agent-session.ts:2416`), so a guest can
/// tell a `--models`-scoped session from an unscoped one. Reading `[]` forever made the two
/// indistinguishable and every model-picking extension free to offer models the user had
/// deliberately excluded. RED before the fix on the seeded assertion.
#[test]
fn scoped_models_reports_the_sessions_real_scoped_set() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);

    // Unscoped is upstream's documented "Empty when no scoping is configured".
    assert_eq!(svc.scoped_models(), json!([]));

    svc.update_scoped_models(vec![
        json!({"model": {"id": "faux-1", "provider": "faux"}, "thinkingLevel": "high"}),
        json!({"model": {"id": "faux-2", "provider": "faux"}}),
    ]);
    let scoped = svc.scoped_models();
    let rows = scoped.as_array().expect("an array");
    assert_eq!(rows.len(), 2, "the whole scoped set reaches the guest: {scoped}");
    assert_eq!(rows[0]["model"]["id"], json!("faux-1"));
    assert_eq!(rows[0]["thinkingLevel"], json!("high"), "pi's per-model thinking level survives");
    assert!(
        rows[1].get("thinkingLevel").is_none(),
        "an unset thinkingLevel is OMITTED, matching an `undefined` field upstream: {scoped}"
    );
}

