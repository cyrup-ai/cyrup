//! `/subagents-inspect-rpc` — the host integration bridge (pi `slash-commands.ts:928-943`).
//!
//! **Signature frozen by the orchestrator; the body is this batch's work.** The request parser
//! already exists (`background/inspect_rpc/request.rs:80-93`) with no registered command; this is
//! the 15-line registration over it.
//!
//! Upstream's two guards are load-bearing and are NOT cosmetic:
//! * in `tui` mode it refuses with an `info` notify naming the interactive alternatives, because
//!   an inspection reply is a widget payload no human can read;
//! * with no UI it returns silently, because there is nowhere to emit.
//!
//! The emit-then-retract pair (`:938-941`) is one handler call: stdio delivers both widget
//! updates in order, the host buffers the payload by request id, and the dedicated key never
//! accumulates visible state.
//!
//! # How `ctx.mode === "tui"` is expressed here, and how exact it is
//!
//! cyrup HAS an exact analogue of upstream's `ctx.mode`: [`cyrup_ext::ExtMode`], carried on
//! [`cyrup_ext::native::HostCtx::mode`] (`crates/cyrup-ext/src/native.rs:129-131`) with the same
//! four values pi's `ExtensionContext.mode` has. What it does NOT have is a route from that field
//! to this handler: the frozen signature takes `has_ui` only, and `dispatch_slash`
//! (`extension/host/slash.rs:380`) threads nothing else.
//!
//! `has_ui` alone cannot stand in for it. The two fields are independent on both sides — upstream
//! tests `ctx.mode === "tui"` FIRST and `!ctx.hasUI` SECOND precisely because a non-TUI surface
//! still has a UI to emit widgets into — and cyrup copies them in side by side from
//! [`cyrup_ext::facade::HostConfig`] (`crates/cyrup-ext/src/host/services.rs:1534-1542`:
//! *"these are not session state — they are fixed host configuration"*). Collapsing them would
//! delete one of upstream's two guards.
//!
//! So the mode is LATCHED: [`record_attached_mode`] is called from
//! [`crate::extension::host::SubagentsExtension`]'s `NativeExtension::execute_command` and
//! `on_event` with the live `ctx.mode`, and [`attached_mode`] reads it back here.
//!
//! **How exact that is, stated precisely.** `HostConfig::mode` is fixed for the life of the host
//! and is copied into every `HostCtx` unchanged, so the latch cannot hold a value that differs
//! from the calling ctx's — and `execute_command` writes it from THIS invocation's ctx immediately
//! before dispatching, so by the time this handler runs the latch holds this call's own mode. The
//! single inexactness is the window before any ctx has been seen (a direct call to this method
//! from a test or a non-`NativeExtension` caller), where the latch answers its seed value
//! [`cyrup_ext::ExtMode::Tui`] — `HostConfig::default()`'s own mode, and the SAFE direction,
//! because that is the arm that refuses rather than the arm that emits. It is a process-wide
//! latch rather than a field on the extension because the mode is process-wide; a per-extension
//! field would have to live on `SubagentsExtension` itself.

use std::sync::atomic::{AtomicU8, Ordering};

use cyrup_ext::ExtMode;
use cyrup_ext::host::WidgetPlacement;

use crate::background::inspect_rpc::{
    INSPECT_WIDGET_KEY, encode_inspect_reply, handle_inspect_rpc_args,
};
use crate::error::SubagentError;
use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
use crate::extension::host::SubagentsExtension;

/// Discriminants for [`ATTACHED_MODE`]. A plain `u8` rather than a `Mutex<ExtMode>`: the value is
/// written on every dispatch and read on one, and an atomic cannot deadlock a handler.
const MODE_TUI: u8 = 0;
const MODE_RPC: u8 = 1;
const MODE_JSON: u8 = 2;
const MODE_PRINT: u8 = 3;

/// The host's attachment mode (pi `ctx.mode`), latched from the live [`cyrup_ext::native::HostCtx`].
///
/// Seeded to [`MODE_TUI`], which is both [`cyrup_ext::facade::HostConfig::default`]'s mode and the
/// conservative answer — see this module's doc.
static ATTACHED_MODE: AtomicU8 = AtomicU8::new(MODE_TUI);

/// Record the mode carried by a live `HostCtx`. Called from the `NativeExtension` impl
/// ([`crate::extension::host::native_impl`]) on every command dispatch and every host event, which
/// is every path that has a ctx at all.
pub(crate) fn record_attached_mode(mode: ExtMode) {
    let encoded = match mode {
        ExtMode::Tui => MODE_TUI,
        ExtMode::Rpc => MODE_RPC,
        ExtMode::Json => MODE_JSON,
        ExtMode::Print => MODE_PRINT,
    };
    ATTACHED_MODE.store(encoded, Ordering::Relaxed);
}

/// The latched mode, decoded. Any unknown discriminant decodes to [`ExtMode::Tui`] for the same
/// reason the seed is `Tui`: the TUI arm refuses, so an unreadable latch cannot leak an inspection
/// payload onto a surface that should not receive one.
#[must_use]
pub(crate) fn attached_mode() -> ExtMode {
    match ATTACHED_MODE.load(Ordering::Relaxed) {
        MODE_RPC => ExtMode::Rpc,
        MODE_JSON => ExtMode::Json,
        MODE_PRINT => ExtMode::Print,
        _ => ExtMode::Tui,
    }
}

impl SubagentsExtension {
    /// pi's `tui`-mode refusal, verbatim (`slash-commands.ts:932`).
    pub(crate) const INSPECT_RPC_TUI_NOTICE: &'static str = "Inspection replies are emitted only on RPC surfaces. Use /subagents or \
         subagent({ action: \"status\", view: \"transcript\" }) interactively.";

    /// pi `slash-commands.ts:930-942`.
    ///
    /// Upstream's handler returns `void` and speaks through `ctx.ui.notify(…, "info")`. This
    /// crate's established mapping for an `info` notify in a slash handler is to RETURN the
    /// sentence as ordinary command output — the same treatment
    /// [`Self::show_fleet`](crate::extension::host::SubagentsExtension)'s already-open arm gives
    /// `:640` — while an `error` notify becomes
    /// [`SubagentError::Management`] (`/subagents-guide`'s usage refusal). Neither guard here is an
    /// error, so both return text: the notice for the `tui` arm, and the empty string for the
    /// no-UI arm, which is upstream's bare `return`.
    pub(crate) async fn slash_subagents_inspect_rpc(
        &self,
        args: &str,
        has_ui: bool,
    ) -> Result<String, SubagentError> {
        // pi `:931-934` — FIRST guard, and first for a reason: a TUI surface has a UI, so the
        // second guard would not catch it and a human would be handed a JSON envelope.
        if attached_mode() == ExtMode::Tui {
            return Ok(Self::INSPECT_RPC_TUI_NOTICE.to_string());
        }
        // pi `:935` — `if (!ctx.hasUI) return;` There is no widget slot to write to, and the reply
        // has no other channel: it is a correlated payload for a host, not prose for a model.
        if !has_ui {
            return Ok(String::new());
        }

        // pi `:936` — `handleInspectRpcArgs(args, { state })`. ALWAYS returns a correlated reply;
        // a parse failure comes back as an `invalid_request` reply rather than an exception, so
        // there is no error arm to write here.
        let roots = self.executor.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, &self.cwd);
        let results_dir = default_results_dir_in(&roots, &self.cwd);
        let current_session =
            crate::identity::SessionId::parse_opt(self.executor.current_session_id().as_deref());
        // The trusted roots are the TRANSCRIPT view's, for the reason
        // `SubagentExecutor::control_inspect` states at length (`extension/executor/status.rs:672`):
        // inspect and `view: "transcript"` dereference the SAME recorded `sessionFile` from the
        // SAME per-cwd artifact roots, so confining them differently would let one surface read a
        // child transcript the other refuses. [CYRUP-DELTA] upstream unions
        // `state.trustedSessionRoots` with `status.sessionRoot` (`inspect-rpc.ts:373-376`), which
        // `RunStatus` does not carry.
        let trusted_roots = vec![
            async_root.clone(),
            crate::artifacts::project_subagents_dir(&self.cwd),
            crate::artifacts::temp_artifacts_dir(&self.cwd),
        ];
        let reply = handle_inspect_rpc_args(
            args,
            &crate::background::inspect_rpc::InspectDeps {
                async_root: &async_root,
                results_dir: &results_dir,
                current_session: current_session.as_ref(),
                trusted_roots: &trusted_roots,
                // ONE clock read, threaded through the replay filter and its re-verification.
                now: crate::time::now_epoch_millis(),
            },
        )
        .await;

        let Some(services) = self.executor.host_services() else {
            // `ctx.ui` is unconditionally present upstream; here the P-1 backend can be unbound
            // (a by-value session, an embedder that granted no capabilities). With nowhere to
            // emit, this is the `!ctx.hasUI` situation arriving one layer down, and it takes the
            // same silent return rather than inventing a second channel for a host-only payload.
            tracing::debug!(
                target: "cyrup_ext_subagents::inspect_rpc",
                "no capability backend bound; the inspect reply has no widget slot to emit into"
            );
            return Ok(String::new());
        };

        // pi `:937-941`, the emit-then-retract pair, in ONE handler call and in upstream's order.
        // Upstream's own comment is the rationale and is reproduced in this module's header:
        // stdio delivers the two widget updates in order, the host buffers the payload by request
        // id, and the dedicated key never accumulates visible state. Splitting them across two
        // dispatches would leave a JSON envelope pinned above the editor.
        //
        // `WidgetPlacement::default()` is `aboveEditor`, upstream's own default for a `setWidget`
        // called with no options (`extensions/types.ts:107-110`).
        let lines = encode_inspect_reply(&reply);
        services.set_widget(INSPECT_WIDGET_KEY, Some(&lines), WidgetPlacement::default());
        services.set_widget(INSPECT_WIDGET_KEY, None, WidgetPlacement::default());
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    // `await_holding_lock`: [`MODE_LOCK`] below serializes the tests that pin the process-wide
    // [`ATTACHED_MODE`] latch, and the guard must outlive the awaited handler call it is pinning
    // the mode for — dropping it early is exactly the race it exists to prevent.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::await_holding_lock
    )]

    use std::sync::{Arc, Mutex, MutexGuard};

    use cyrup_ext::host::{HostServices, WidgetPlacement};

    use super::*;
    use crate::registration::SubagentExtensionConfig;

    /// A recording capability backend: the only host effect this command has is `set_widget`, so
    /// that is the only method overridden.
    #[derive(Default)]
    struct WidgetRecorder {
        calls: Mutex<Vec<(String, Option<Vec<String>>)>>,
    }

    impl WidgetRecorder {
        fn calls(&self) -> Vec<(String, Option<Vec<String>>)> {
            self.calls
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|e| e.into_inner().clone())
        }
    }

    impl HostServices for WidgetRecorder {
        fn set_widget(&self, key: &str, lines: Option<&[String]>, _placement: WidgetPlacement) {
            if let Ok(mut g) = self.calls.lock() {
                g.push((key.to_string(), lines.map(<[String]>::to_vec)));
            }
        }
    }

    /// Serializes every test that pins [`ATTACHED_MODE`]. The latch is process-wide because the
    /// host's mode is; under `cargo test` (one process, many threads) two mode-pinning tests would
    /// otherwise interleave. `nextest`'s process-per-test already isolates them — this makes the
    /// tests correct under both runners.
    static MODE_LOCK: Mutex<()> = Mutex::new(());

    /// Build an extension over a scratch cwd with a recording backend bound, and pin the latched
    /// mode for as long as the returned guard is held.
    fn harness(
        mode: ExtMode,
    ) -> (
        tempfile::TempDir,
        SubagentsExtension,
        Arc<WidgetRecorder>,
        MutexGuard<'static, ()>,
    ) {
        let guard = MODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig::default(),
            dir.path().to_path_buf(),
        );
        let recorder = Arc::new(WidgetRecorder::default());
        let services: Arc<dyn HostServices> = recorder.clone();
        ext.executor().set_host_services(services);
        record_attached_mode(mode);
        (dir, ext, recorder, guard)
    }

    /// MUTATION: deleting the `ctx.mode === "tui"` guard (`slash-commands.ts:931-934`), or
    /// collapsing it into `has_ui`. In TUI mode the command must answer with upstream's notice and
    /// emit NOTHING — an inspection reply is a JSON envelope for a host, and a human handed one
    /// has been shown a bug.
    #[tokio::test]
    async fn the_tui_guard_returns_the_notice_and_emits_no_widget() {
        let (_dir, ext, recorder, _mode) = harness(ExtMode::Tui);
        let out = ext
            .slash_subagents_inspect_rpc("req1 asyncrun00001", true)
            .await
            .expect("the notice is output, not an error");
        assert_eq!(out, SubagentsExtension::INSPECT_RPC_TUI_NOTICE);
        assert!(
            recorder.calls().is_empty(),
            "the tui guard returns BEFORE any widget write: {:?}",
            recorder.calls()
        );
    }

    /// MUTATION: deleting the `!ctx.hasUI` guard (`:935`), or reordering it ahead of the mode
    /// guard. With no UI there is nowhere to emit, so the command returns silently — no notice
    /// (that is the other guard's sentence) and no widget.
    #[tokio::test]
    async fn the_no_ui_guard_returns_silently_and_emits_no_widget() {
        let (_dir, ext, recorder, _mode) = harness(ExtMode::Rpc);
        let out = ext
            .slash_subagents_inspect_rpc("req1 asyncrun00001", false)
            .await
            .expect("a silent return is not an error");
        assert_eq!(out, "", "upstream's bare `return` carries no text");
        assert_ne!(
            out,
            SubagentsExtension::INSPECT_RPC_TUI_NOTICE,
            "the no-UI arm must not borrow the tui arm's sentence"
        );
        assert!(
            recorder.calls().is_empty(),
            "nothing is emitted with no UI: {:?}",
            recorder.calls()
        );
    }

    /// MUTATION: dropping the retraction (`:941`), emitting the two updates from two dispatches,
    /// or writing them to different keys. Exactly two operations must land on
    /// [`INSPECT_WIDGET_KEY`], in upstream's order, the second clearing the slot — otherwise the
    /// JSON envelope stays pinned above the editor forever.
    #[tokio::test]
    async fn a_request_emits_exactly_two_widget_operations_the_second_a_retraction() {
        let (_dir, ext, recorder, _mode) = harness(ExtMode::Rpc);
        ext.slash_subagents_inspect_rpc("req1 asyncrun00001", true)
            .await
            .expect("a well-formed request always produces a correlated reply");

        let calls = recorder.calls();
        assert_eq!(calls.len(), 2, "emit-then-retract is two calls: {calls:?}");
        assert_eq!(calls[0].0, INSPECT_WIDGET_KEY);
        assert_eq!(calls[1].0, INSPECT_WIDGET_KEY);
        let emitted = calls[0]
            .1
            .as_ref()
            .expect("the first call SETS the payload");
        assert_eq!(emitted.len(), 1, "one envelope line: {emitted:?}");
        assert!(
            emitted[0].starts_with(crate::background::inspect_rpc::INSPECT_WIDGET_PREFIX),
            "the payload is `encode_inspect_reply`'s envelope: {emitted:?}"
        );
        assert!(
            emitted[0].contains("\"req1\""),
            "the reply is correlated to the caller's request id: {emitted:?}"
        );
        assert!(
            calls[1].1.is_none(),
            "the second call RETRACTS the widget: {:?}",
            calls[1].1
        );
    }

    /// MUTATION: treating a parse failure as an error return. `handle_inspect_rpc_args` always
    /// answers with a correlated reply (`inspect-rpc.ts:433-443`), so a malformed request still
    /// emits and retracts — dropping that would leave the host waiting forever for a request id it
    /// sent.
    #[tokio::test]
    async fn a_malformed_request_still_emits_and_retracts_a_correlated_reply() {
        let (_dir, ext, recorder, _mode) = harness(ExtMode::Json);
        ext.slash_subagents_inspect_rpc("req9 --nope", true)
            .await
            .expect("a parse failure is a REPLY, not an error");
        let calls = recorder.calls();
        assert_eq!(calls.len(), 2, "still emit-then-retract: {calls:?}");
        let emitted = calls[0].1.as_ref().expect("payload");
        assert!(
            emitted[0].contains("invalid_request"),
            "the failure rides back as an error reply: {emitted:?}"
        );
        assert!(
            emitted[0].contains("\"req9\""),
            "and it is still correlated: {emitted:?}"
        );
    }

    /// MUTATION: seeding or decoding [`ATTACHED_MODE`] to a non-TUI value. The conservative
    /// direction is the refusing one; an unknown discriminant must not open the emitting path.
    #[test]
    fn the_mode_latch_round_trips_and_decodes_unknowns_as_tui() {
        let _guard = MODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for mode in [ExtMode::Tui, ExtMode::Rpc, ExtMode::Json, ExtMode::Print] {
            record_attached_mode(mode);
            assert_eq!(attached_mode(), mode);
        }
        ATTACHED_MODE.store(u8::MAX, Ordering::Relaxed);
        assert_eq!(
            attached_mode(),
            ExtMode::Tui,
            "an unreadable latch refuses rather than emits"
        );
        record_attached_mode(ExtMode::Tui);
    }
}
