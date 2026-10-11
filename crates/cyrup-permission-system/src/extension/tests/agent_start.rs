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
            .on_event(
                &HostEvent::SessionStart {
                    reason: "startup".to_string(),
                    previous_session_file: None,
                },
                &ctx,
            )
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
            .on_event(
                &HostEvent::SessionStart {
                    reason: "reload".to_string(),
                    previous_session_file: None,
                },
                &ctx,
            )
            .await;
        let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
        assert_eq!(
            guard(&host.active_tools).last().cloned(),
            Some(vec!["read".to_string()]),
            "a tool-level bash deny must withhold `bash` (PERM-009's rule, re-applied)"
        );
    });
}

/// The shaping replaces the active set with the names it hands `set_active_tools`, so it must not
/// name a tool that registration does not activate: a `codemode`/`deferred` tool would be declared
/// to the model on the next request, a `hidden` one is unreachable by definition. A `deferred` tool
/// that something already loaded (so it is active) stays, or shaping would unload it every turn.
#[test]
fn shaping_never_activates_codemode_deferred_or_hidden_tools() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let host = Arc::new(ExposureRegistry {
            rows: vec![
                ("read".to_string(), "direct"),
                ("ask_user".to_string(), "model-only"),
                ("docs_search".to_string(), "codemode"),
                ("loaded_later".to_string(), "deferred"),
                ("still_deferred".to_string(), "deferred"),
                ("internal".to_string(), "hidden"),
            ],
            active: vec!["read".to_string(), "loaded_later".to_string()],
            applied: std::sync::Mutex::new(Vec::new()),
        });
        ext.set_host_services(host.clone());
        let _ = ext
            .on_event(&before_agent_start("SYSTEM"), &event_ctx(agent_dir))
            .await;

        let applied = guard(&host.applied).clone();
        assert_eq!(applied.len(), 1, "one shaping call: {applied:?}");
        assert_eq!(
            applied[0],
            vec!["read", "ask_user", "loaded_later"],
            "only declarable tools and tools already active may be named"
        );
    });
}

/// The prompt the shaping returns describes the tools it has just applied, not the ones that were
/// active when the handler was called. The hook is handed the base built for the old set; applying
/// the new set rebuilds the base, and the replacement a handler returns wins over that rebuild, so
/// a replacement derived from the stale text told the model about tools it no longer had and
/// nothing about the ones it now had.
#[test]
fn the_sanitized_prompt_is_derived_from_the_prompt_of_the_tools_it_applied() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        // `write` is not in the registry, so its guideline is removed from whatever is sanitized.
        let stale = "STALE\n\nGuidelines:\n- use write only for new files or complete rewrites\n- keep stale\n";
        let rebuilt = "REBUILT\n\nGuidelines:\n- use write only for new files or complete rewrites\n- keep rebuilt\n";
        let host = Arc::new(RebuildingHost {
            names: vec!["bash".to_string(), "read".to_string()],
            prompt: std::sync::Mutex::new(stale.to_string()),
            rebuilt: rebuilt.to_string(),
            applied: std::sync::Mutex::new(Vec::new()),
        });
        ext.set_host_services(host.clone());

        let outcome = ext
            .on_event(&before_agent_start(stale), &event_ctx(agent_dir))
            .await;
        let prompt =
            replaced_prompt(outcome).expect("a sanitized prompt is returned as a mutation");
        assert_eq!(guard(&host.applied).len(), 1, "the tools were applied");
        assert_eq!(prompt, "REBUILT\n\nGuidelines:\n- keep rebuilt");
    });
}

/// Another handler ahead of the shaping may have edited the prompt; reading the rebuilt base back
/// would drop that edit, so then the prompt it was handed is the one that is sanitized.
#[test]
fn a_prompt_an_earlier_handler_changed_is_sanitized_as_it_was_handed_over() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let base = "BASE\n\nGuidelines:\n- use write only for new files or complete rewrites\n";
        let edited = "BASE EDITED BY ANOTHER HANDLER\n\nGuidelines:\n- use write only for new files or complete rewrites\n- keep edited\n";
        let host = Arc::new(RebuildingHost {
            names: vec!["bash".to_string(), "read".to_string()],
            prompt: std::sync::Mutex::new(base.to_string()),
            rebuilt: "REBUILT\n".to_string(),
            applied: std::sync::Mutex::new(Vec::new()),
        });
        ext.set_host_services(host.clone());

        let outcome = ext
            .on_event(&before_agent_start(edited), &event_ctx(agent_dir))
            .await;
        let prompt =
            replaced_prompt(outcome).expect("a sanitized prompt is returned as a mutation");
        assert_eq!(
            prompt,
            "BASE EDITED BY ANOTHER HANDLER\n\nGuidelines:\n- keep edited"
        );
    });
}

/// [CYRUP-DELTA] A session replacement (`/new`, RPC `new_session`, a second ACP `session/new`) builds
/// the replacement first, binds its `LiveHostServices` to the SAME extension object, shuts the outgoing
/// session down and starts the new one. The extension kept the first backend it was given (a set-once
/// slot), so after the replacement it shaped the tool set on the replaced session's registry: the new
/// session kept the tools the first one had before the shaping (`codemode`, `grep`, `find`, `ls` gone,
/// a denied tool declared again) and the gate alone enforced the policy.
#[test]
fn a_replacement_session_gets_the_tool_set_shaped_on_its_own_backend() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let ctx = event_ctx(agent_dir.clone());

        let first = Arc::new(LifecycleRecorder::new());
        ext.set_host_services(first.clone());
        let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
        let shaped = guard(&first.active_tools).clone();
        assert_eq!(shaped.len(), 1, "the first session is shaped: {shaped:?}");

        // The replacement: its backend is bound before the outgoing session shuts down.
        let second = Arc::new(LifecycleRecorder::new());
        ext.set_host_services(second.clone());
        let _ = ext
            .on_event(
                &HostEvent::SessionShutdown {
                    reason: "new".to_string(),
                    target_session_file: None,
                },
                &ctx,
            )
            .await;
        let _ = ext
            .on_event(
                &HostEvent::SessionStart {
                    reason: "new".to_string(),
                    previous_session_file: None,
                },
                &ctx,
            )
            .await;
        let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;

        assert_eq!(
            *guard(&second.active_tools),
            shaped,
            "the replacement session declares the tools the first one did"
        );
        assert_eq!(
            guard(&first.active_tools).len(),
            1,
            "the replaced session is not shaped again"
        );
    });
}
