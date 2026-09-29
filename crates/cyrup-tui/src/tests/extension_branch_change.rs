//! EXT-064 — the branch-change PUSH reaches a subscribed extension from the run loop's own poll arm.
//!
//! **Upstream.** pi's custom footer is a COMPONENT the draw path re-renders, so it re-reads
//! `getGitBranch()` every frame and a branch change repaints it with no extension code involved
//! (`modes/interactive/interactive-mode.ts:1046` re-requests the render from the mode's OWN
//! `onBranchChange` subscription). The extension's access to the same signal is
//! `ReadonlyFooterDataProvider.onBranchChange(callback)`
//! (`core/footer-data-provider.ts:139-143` @v0.87.1), fired from `notifyBranchChange` — which pi
//! calls only inside `if (this.cachedBranch !== next)` (`:224-227`).
//!
//! **cyrup.** A closure cannot cross a component boundary, so a guest hands the host rendered TEXT
//! once; without this push its footer would freeze at whatever branch was current when it called
//! `set-footer`. [`App::poll_footer_git_branch`] is cyrup's `refreshGitBranchAsync` and already
//! answers `true` only on a REAL change, so it is the exact site upstream fires from — which is why
//! this test drives the ARM (`App::on_git_branch_poll`) rather than `ExtensionHost::branch_change`
//! directly. `cyrup-ext`'s `tests::branch_change` covers the host fan-out; this covers the wiring
//! between them, which is the half a host-level test cannot see.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{App, UiTheme};
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{SessionBuilder, SessionConfig};

/// The guest stand-in: subscribes, and records every branch it is pushed.
struct BranchWatcher {
    seen: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait::async_trait]
impl NativeExtension for BranchWatcher {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("ext-064-branch-watcher")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe_branch_change();
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn on_branch_change(&self, branch: Option<&str>) {
        if let Ok(mut g) = self.seen.lock() {
            g.push(branch.map(str::to_string));
        }
    }
}

/// A `.git` directory holding `HEAD` — enough for the footer, which READS HEAD rather than shelling
/// out (pi `resolveGitBranchSync`), so the test needs no `git` binary.
fn write_head(root: &Path, head: &str) {
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git").join("HEAD"), head).unwrap();
}

/// `App::run`'s own backend: the poll arm lives on `impl App<InlineBackend<TuiStdout>>`.
fn inline_app() -> App<crate::InlineBackend<crate::TuiStdout>> {
    App::new(
        crate::InlineBackend::with_anchor(
            crate::write_log::tui_stdout(),
            ratatui::layout::Position::ORIGIN,
        ),
        UiTheme::dark(),
    )
    .unwrap()
}

/// The run loop's context, built exactly as `App::run` builds it. The arm under test reads only
/// `ctx.session`; the rest are the channels/timers the struct requires.
fn run_ctx(session: Arc<cyrup_session_svc::AgentSession>) -> crate::app::RunCtx {
    let mut spinner = tokio::time::interval(Duration::from_millis(80));
    spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    crate::app::RunCtx {
        session,
        runtime: None,
        cancel: cyrup_core::CancelToken::new(),
        gen_rx: None,
        spinner,
        overlay_tick: None,
        bash_rx: None,
        package_update_rx: None,
        ui_tx: tokio::sync::mpsc::unbounded_channel().0,
        ui_effect_tx: tokio::sync::mpsc::unbounded_channel().0,
        ext_error_tx: tokio::sync::mpsc::unbounded_channel().0,
        commands_changed_tx: tokio::sync::mpsc::unbounded_channel().0,
        overlay_tx: tokio::sync::mpsc::unbounded_channel().0,
        theme_switch_tx: tokio::sync::mpsc::unbounded_channel().0,
        shortcut_status_tx: tokio::sync::mpsc::unbounded_channel().0,
    }
}

/// THE proof: a checkout under the session's cwd reaches a subscribed extension, once, with the new
/// branch — and an UNCHANGED poll reaches it not at all.
///
/// **RED without the change:** `App::on_git_branch_poll` requested a repaint and returned; nothing
/// in the tree ever told an extension the branch had moved, and `world.wit`'s `[CYRUP-DELTA]` said
/// so in as many words — "`onBranchChange(callback)` … is still uncovered: it is a PUSH
/// subscription, which no import signature can express — it needs an event EXPORT … and is filed as
/// its own row rather than smuggled in here". Deleting the `ext_host.branch_change(...)` call from
/// that arm leaves `seen` empty and fails the first assertion. Observed RED by doing exactly that.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkout_pushes_the_new_branch_to_a_subscribed_extension() {
    let tmp = tempfile::Builder::new()
        .prefix("cyrup-ext-064-branch-")
        .tempdir()
        .unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_head(&cwd, "ref: refs/heads/main\n");

    let seen = Arc::new(Mutex::new(Vec::new()));
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let mut config = SessionConfig::new(cwd.clone(), agent_dir);
    config.trust_override = Some(true);
    config.no_extensions = true;
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config)
        .with_native_extension(
            Arc::new(BranchWatcher { seen: seen.clone() }) as Arc<dyn NativeExtension>
        )
        .build()
        .await
        .unwrap()
        .into_shared();

    let mut app = inline_app();
    app.set_footer_git_cwd(&cwd);
    let mut ctx = run_ctx(session);

    // (a) An unchanged repo pushes NOTHING. pi fires its callbacks only inside
    //     `if (this.cachedBranch !== next)` (`core/footer-data-provider.ts:224-227` @v0.87.1), so a
    //     poll that found nothing moved must be silent — an extension that re-rendered its footer
    //     on every tick would repaint the screen at the poll interval forever.
    app.on_git_branch_poll(&mut ctx).await.unwrap();
    assert!(
        seen.lock().unwrap().is_empty(),
        "an unchanged poll notified a subscriber: {:?}",
        seen.lock().unwrap()
    );

    // (b) A real checkout pushes exactly one notification, carrying the NEW branch.
    write_head(&cwd, "ref: refs/heads/feature/x\n");
    app.on_git_branch_poll(&mut ctx).await.unwrap();
    assert_eq!(
        seen.lock().unwrap().clone(),
        vec![Some("feature/x".to_string())],
        "the checkout reached the extension with the new branch (pi `notifyBranchChange`, \
         `core/footer-data-provider.ts:197-199` @v0.87.1)"
    );

    // (c) …and polling again with nothing moved stays silent, so the push tracks CHANGES rather
    //     than ticks.
    app.on_git_branch_poll(&mut ctx).await.unwrap();
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "a second unchanged poll pushed again: {:?}",
        seen.lock().unwrap()
    );
}
