//! The `before_agent_start` context-hygiene layer's caching: shape once, re-shape when the policy
//! actually changed.

use std::sync::Arc;

use cyrup_ext::{HostEvent, NativeExtension};

use super::support::*;
use crate::extension::paths::POLICY_FILE;
use crate::extension::{PermissionSystemExtension, guard};

/// PERM-013 (RED before the fix). pi calls `setActiveTools` ONLY when the active-tools cache key
/// changed (v0.8.0 `index.ts:1894-1898`) and short-circuits the two sanitizers on a prompt-state
/// key hit (`:1908-1913`). Cyrup recomputed and re-applied everything on every turn.
#[test]
fn repeated_before_agent_start_applies_the_active_tool_set_once() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let host = Arc::new(LifecycleRecorder::new());
        ext.set_host_services(host.clone());
        let ctx = event_ctx(agent_dir.clone());

        for _ in 0..3 {
            let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
        }
        assert_eq!(
            guard(&host.active_tools).len(),
            1,
            "an unchanged policy + registry must apply the tool set exactly once (pi `:1895`)"
        );

        // A DIFFERENT system prompt changes the prompt-state key but not the tools key, so the
        // sanitizers re-run while `setActiveTools` still does not.
        let _ = ext.on_event(&before_agent_start("SYSTEM v2"), &ctx).await;
        assert_eq!(guard(&host.active_tools).len(), 1);

        // A session_start invalidates the whole cache (pi `invalidateAgentStartCache`,
        // `index.ts:1823`), so the next turn re-applies.
        let _ = ext
            .on_event(&HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }, &ctx)
            .await;
        let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
        assert_eq!(
            guard(&host.active_tools).len(),
            2,
            "the cache must be invalidated by session_start"
        );
    });
}

/// PERM-013's correctness hinge: a mid-session POLICY edit must invalidate the cached prompt
/// state even though prompt / cwd / registry are unchanged. That is why
/// `PermissionManager::policy_cache_stamp` is public upstream (`permission-manager.ts:781`).
#[test]
fn a_mid_session_policy_edit_re_applies_the_shaped_tool_set() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let host = Arc::new(LifecycleRecorder::new());
        ext.set_host_services(host.clone());
        let ctx = event_ctx(agent_dir.clone());

        let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
        assert_eq!(guard(&host.active_tools).last().map(Vec::len), Some(2));

        // Deny `bash` at the tool level; the exposed set must shrink on the NEXT turn.
        write_file(&agent_dir.join(POLICY_FILE), r#"{"tools":{"bash":"deny"}}"#);
        // The manager is rebuilt at session_start / resources_discover, matching pi — a policy
        // edit takes effect through the same reload path an operator triggers.
        let _ = ext
            .on_event(&HostEvent::SessionStart { reason: "reload".to_string(), previous_session_file: None }, &ctx)
            .await;
        let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
        assert_eq!(
            guard(&host.active_tools).last().cloned(),
            Some(vec!["read".to_string()]),
            "a tool-level bash deny must withhold `bash` (PERM-009's rule, re-applied)"
        );
    });
}
