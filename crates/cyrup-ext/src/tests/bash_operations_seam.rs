//! DRIFT-004 — the GUEST tier of pi's `UserBashEventResult.operations`
//! (`packages/coding-agent/src/core/extensions/types.ts:1139` @v0.84.4, the `BashOperations`
//! interface at `core/tools/bash.ts:63-81`), host side.
//!
//! The native tier is covered by `cyrup-session-svc`'s `..._operations_override_...` tests and by
//! [`crate::tests::payload_and_seam_parity`]; what this file pins is everything the guest tier adds:
//! the registry table that says WHICH extension declared a backend, the keyed back-channel the two
//! closure-shaped `exec` options travel over (`host-bash.emit-bash-output` /
//! `is-bash-cancelled`), and the two-halves rule in
//! [`crate::ExtensionHost::user_bash_operations`]. The full wasmtime round trip — a real component
//! whose `bash-operations-exec` export runs a command and streams it back — is
//! `crates/cyrup-it/tests/ext/wasm_bash_operations.rs`, which needs a built guest and so cannot
//! live in this in-process suite.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::registry::ExtensionRegistry;
use cyrup_core::ExtensionId;

/// The registry answers PER OWNER, because upstream reads `operations` off exactly one result —
/// the first truthy handler's (`extensions/runner.ts:1005-1032`) — so "some extension has a
/// backend" is never the question being asked.
#[test]
fn the_registry_records_a_bash_backend_per_owner() {
    let reg = ExtensionRegistry::new();
    let ssh = ExtensionId::from("ssh");
    let other = ExtensionId::from("other");

    assert!(!reg.has_bash_operations(&ssh).unwrap());
    reg.register_bash_operations(ssh.clone()).unwrap();
    assert!(reg.has_bash_operations(&ssh).unwrap());
    assert!(
        !reg.has_bash_operations(&other).unwrap(),
        "one extension's declaration must not lend a backend to another"
    );

    // Idempotent: the declaration is a fact about the guest, not a fold step (the same rule
    // `register_markdown_transformer` follows).
    reg.register_bash_operations(ssh.clone()).unwrap();
    assert!(reg.has_bash_operations(&ssh).unwrap());
}

/// A guest that never declared a backend gets none, and — the half that is easy to get wrong — a
/// guest that DID declare one but has no live instance also gets none, rather than a forwarder
/// whose export call would fail.
#[tokio::test]
async fn user_bash_operations_needs_both_the_declaration_and_a_live_instance() {
    let host = crate::ExtensionHost::new(crate::HostConfig {
        mode: crate::ExtMode::Rpc,
        has_ui: false,
        cwd: std::path::PathBuf::from("."),
    });
    let ghost = ExtensionId::from("declared-but-not-loaded");

    assert!(
        host.user_bash_operations(&ghost, "uname -a", false, "/tmp")
            .is_none(),
        "an owner with no declaration is upstream's absent `operations`"
    );

    host.registry().register_bash_operations(ghost.clone()).ok();
    assert!(
        host.user_bash_operations(&ghost, "uname -a", false, "/tmp")
            .is_none(),
        "a declaration with no live instance must still fall through to \
         `createLocalBashOperations` (`core/agent-session.ts:2782`'s `??`), not to a forwarder \
         with nothing to forward to"
    );
}

/// The `host-bash` back-channel is keyed, and the key is load-bearing: pi's `onData` and `signal`
/// ARE the call, so a chunk cannot reach another command's sink by construction. cyrup's queue is
/// instance-scoped, so the `call-id` is what re-imposes that (EXT-M06's rule, applied here).
#[cfg(feature = "wasm-host")]
#[test]
fn bash_output_and_cancellation_are_keyed_to_the_live_command() {
    use crate::host::services::{DenyServices, GuestState};
    use cyrup_core::CancelToken;
    use std::sync::Arc;

    let guest = GuestState::with_services(
        ExtensionId::from("ssh"),
        Arc::new(ExtensionRegistry::new()),
        Arc::new(DenyServices),
    );

    guest.push_bash_output("call-1".into(), b"one".to_vec());
    guest.push_bash_output("call-2".into(), b"stray".to_vec());
    guest.push_bash_output("call-1".into(), b"two".to_vec());

    let mine = guest.take_bash_output_for("call-1");
    assert_eq!(mine, vec![b"one".to_vec(), b"two".to_vec()], "in order");
    assert_eq!(
        guest.clear_bash_output(),
        0,
        "the foreign chunk is DROPPED by the drain, not left to surface in the next command"
    );

    // The `signal` poll: bound to one call id, and false for anything else.
    let cancel = CancelToken::new();
    guest.set_bash_cancel(Some(("call-1".into(), cancel.clone())));
    assert!(!guest.bash_is_cancelled("call-1"));
    cancel.cancel();
    assert!(guest.bash_is_cancelled("call-1"));
    assert!(
        !guest.bash_is_cancelled("call-2"),
        "a stale or forged id must never read another command's cancellation"
    );

    guest.set_bash_cancel(None);
    assert!(
        !guest.bash_is_cancelled("call-1"),
        "unbinding is what stops a finished command's token answering a later poll"
    );
}

/// The drop guard is what makes the two properties above hold on the exit paths that never reach a
/// replay — a cancelled command, or the exec future being dropped at an await point.
#[cfg(feature = "wasm-host")]
#[test]
fn the_drop_guard_unbinds_the_command_and_discards_its_unreplayed_output() {
    use crate::host::live::BashCallBinding;
    use crate::host::services::{DenyServices, GuestState};
    use cyrup_core::CancelToken;
    use std::sync::Arc;

    let guest = Arc::new(GuestState::with_services(
        ExtensionId::from("ssh"),
        Arc::new(ExtensionRegistry::new()),
        Arc::new(DenyServices),
    ));
    let cancel = CancelToken::new();
    cancel.cancel();
    {
        let _binding = BashCallBinding(&guest);
        guest.set_bash_cancel(Some(("call-1".into(), cancel)));
        guest.push_bash_output("call-1".into(), b"partial".to_vec());
        assert!(guest.bash_is_cancelled("call-1"));
    }
    assert!(
        !guest.bash_is_cancelled("call-1"),
        "the guard unbinds the token"
    );
    assert_eq!(
        guest.clear_bash_output(),
        0,
        "the guard discarded the chunk that was never replayed"
    );
}
