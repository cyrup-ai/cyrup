//! ghostty's [`InspectorPlugin`] implementation.
//!
//! Upstream: `src/inspectors/ghostty/plugin.ts` (17 lines @v0.68.0) — three members, and the two
//! that are NOT here are the whole point of this backend's honesty:
//!
//! * `owns: () => false` (`:14`) — ghostty writes no binding, so it is **never** the owner an
//!   `inspector.status`/`inspector.close` lookup finds (`actions.ts:138`). A target with a ghostty
//!   terminal open and no herdr binding answers upstream's non-error *"No inspector plugin owns
//!   this binding for async run {id}."*, and [`super::actions::open_ghostty_inspector`]'s own
//!   success sentence says so up front.
//! * `status`/`close` are absent from the object entirely, which is [`None`] here.
//!
//! # `available()` is two reads, and neither of them probes
//!
//! `ghostty/plugin.ts:13` is `platform === "darwin" && context.env.TERM_PROGRAM?.toLowerCase() ===
//! "ghostty"`. No `osascript --version`, no `tell application`, no spawn — so on a Linux box (this
//! container, every CI runner) `inspector.open` refuses in microseconds and **cannot hang**. The
//! Ghostty-1.3/Automation sentence users see on failure is the *failure hint*
//! ([`super::actions::GHOSTTY_FAILURE_HINT`], `ghostty/actions.ts:52`), appended after a real
//! attempt — it is not a precondition this gate checks.
//!
//! # Where each half of the gate is injected
//!
//! Upstream injects `platform` and `runner` through `GhosttyPluginDeps` (`:4-7`) and reads the
//! environment off `context.env`. cyrup does the same, now that the contract carries
//! [`InspectorContext::env`]: `TERM_PROGRAM` is read off the CONTEXT — so the dispatcher can drive
//! this gate — while `platform` stays injectable on the plugin
//! ([`GhosttyInspectorPlugin::with_macos`]), exactly as upstream's `deps.platform` is. The frozen
//! [`Platform`](crate::inspectors::types::Platform) is not the place for it: it is the
//! shell-quoting pair `Win32`/`Unix` with no `Darwin` member, it belongs to
//! `format_shell_command`, and widening it would change a different module's exhaustive match.
//!
//! That is what makes T-GHOST-1 a plain unit test on Linux, exactly as upstream's own tests are
//! off macOS.

use std::sync::Arc;

use cyrup_core::{ToolError, ToolResult};

use super::actions::{self, OsascriptRunner};
use crate::inspectors::plugins::{GhosttyRunner, InspectorPlugin};
use crate::inspectors::types::{InspectorContext, InspectorLaunch, InspectorParams};

/// The ghostty backend.
#[derive(Clone, Default)]
pub struct GhosttyInspectorPlugin {
    /// Injected seam. `None` ⇒ the real [`OsascriptRunner`]. pi's `deps.runner` (`:6`).
    runner: Option<Arc<dyn GhosttyRunner>>,
    /// Injected platform. `None` ⇒ `cfg!(target_os = "macos")`. pi's `deps.platform ??
    /// process.platform` (`:10`).
    macos: Option<bool>,
}

impl std::fmt::Debug for GhosttyInspectorPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GhosttyInspectorPlugin")
            .field("runner", &self.runner.as_ref().map(|_| "injected"))
            .field("macos", &self.macos)
            .finish()
    }
}

impl GhosttyInspectorPlugin {
    /// Construct the backend.
    ///
    /// `const` and argument-free because
    /// [`crate::inspectors::plugins::builtin_inspector_plugins`] — which is in the frozen contract
    /// — calls exactly that. Nothing is resolved, spawned or read here.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            runner: None,
            macos: None,
        }
    }

    /// Inject the `osascript` seam. pi's `deps.runner` (`ghostty/plugin.ts:6`).
    #[must_use]
    pub fn with_runner(mut self, runner: Arc<dyn GhosttyRunner>) -> Self {
        self.runner = Some(runner);
        self
    }

    /// Inject the platform half of the gate. pi's `deps.platform` (`ghostty/plugin.ts:5,10`).
    #[must_use]
    pub const fn with_macos(mut self, macos: bool) -> Self {
        self.macos = Some(macos);
        self
    }

    /// `platform === "darwin"` (`:13`), injectable.
    fn is_macos(&self) -> bool {
        self.macos.unwrap_or(cfg!(target_os = "macos"))
    }

    /// `context.env.TERM_PROGRAM` (`:13`) — off the VERB's environment, which production fills
    /// from `std::env::vars()` (`inspectors::actions::process_env`). Absent is `None`, exactly as
    /// an unset variable is `undefined` upstream.
    fn term_program(ctx: &InspectorContext) -> Option<&str> {
        ctx.env.get("TERM_PROGRAM").map(String::as_str)
    }

    /// The seam to run: the injected one, else the real `osascript`.
    fn seam(&self) -> Arc<dyn GhosttyRunner> {
        match &self.runner {
            Some(runner) => Arc::clone(runner),
            None => Arc::new(OsascriptRunner::new()),
        }
    }
}

#[async_trait::async_trait]
impl InspectorPlugin for GhosttyInspectorPlugin {
    fn name(&self) -> &'static str {
        "ghostty"
    }

    async fn available(&self, ctx: &InspectorContext) -> bool {
        // `?.toLowerCase() === "ghostty"` (`:13`): case-insensitive, and an absent variable is
        // false rather than a panic. No probe — see this module's doc.
        self.is_macos()
            && Self::term_program(ctx).is_some_and(|value| value.eq_ignore_ascii_case("ghostty"))
    }

    fn owns(&self, _ctx: &InspectorContext) -> bool {
        // pi `owns: () => false` (`:14`), verbatim. This backend writes no binding, so it can
        // never claim one.
        false
    }

    async fn open(
        &self,
        ctx: &InspectorContext,
        launch: &InspectorLaunch,
        params: &InspectorParams,
    ) -> Result<ToolResult, ToolError> {
        actions::open_ghostty_inspector(ctx, launch, params, self.seam().as_ref()).await
    }

    async fn status(&self, _ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>> {
        // pi's plugin object has no `status` key (`:11-16`). `None` is that absence — and it is
        // NOT reachable through `builtin_inspector_plugins()`, because `owns()` is `false` and the
        // dispatcher only asks the OWNER (`actions.ts:138`). A third-party plugin could reach it.
        None
    }

    async fn close(&self, _ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>> {
        None
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;
    use crate::inspectors::types::{CommandOutput, HerdrErrorCode, InspectorTarget};

    #[derive(Default)]
    struct RecordingRunner {
        calls: Mutex<Vec<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl GhosttyRunner for RecordingRunner {
        async fn run(&self, args: &[&str]) -> Result<CommandOutput, HerdrErrorCode> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|a| (*a).to_owned()).collect());
            Ok(CommandOutput {
                stdout: "term-1".to_owned(),
                stderr: String::new(),
                exit_code: Some(0),
            })
        }
    }

    /// A context carrying `env`, which is where the `TERM_PROGRAM` half of the gate now lives.
    fn context_with(dir: &Path, env: BTreeMap<String, String>) -> InspectorContext {
        InspectorContext {
            target: InspectorTarget {
                run_id: "run-1".to_owned(),
                async_dir: dir.to_path_buf(),
                child_index: None,
            },
            trusted_dir: dir.to_path_buf(),
            mission_id: None,
            mission_path: None,
            env,
        }
    }

    /// Inside a Ghostty terminal.
    fn context(dir: &Path) -> InspectorContext {
        context_with(dir, ghostty_env())
    }

    fn ghostty_env() -> BTreeMap<String, String> {
        BTreeMap::from([("TERM_PROGRAM".to_owned(), "Ghostty".to_owned())])
    }

    /// pi `ghostty/plugin.ts:13`, both conjuncts and the case fold.
    ///
    /// GUT either conjunct and this backend claims to be available on every Linux box, which turns
    /// `inspector.open`'s instant honest refusal into an `osascript` spawn against a path that is
    /// not there. GUT the `to_ascii_lowercase` and a terminal reporting `ghostty` (the spelling
    /// Ghostty itself exports on some builds) is refused.
    #[tokio::test]
    async fn available_is_darwin_plus_a_case_folded_term_program() {
        let dir = tempfile::tempdir().unwrap();

        let linux = GhosttyInspectorPlugin::new().with_macos(false);
        assert!(!linux.available(&context(dir.path())).await);

        let macos = GhosttyInspectorPlugin::new().with_macos(true);
        assert!(
            !macos
                .available(&context_with(dir.path(), BTreeMap::new()))
                .await
        );
        assert!(
            !macos
                .available(&context_with(
                    dir.path(),
                    BTreeMap::from([("TERM_PROGRAM".to_owned(), "iTerm.app".to_owned())]),
                ))
                .await
        );

        for spelling in ["Ghostty", "ghostty", "GHOSTTY"] {
            let ctx = context_with(
                dir.path(),
                BTreeMap::from([("TERM_PROGRAM".to_owned(), spelling.to_owned())]),
            );
            assert!(macos.available(&ctx).await, "{spelling}");
        }
    }

    /// `available()` must not spawn. GUT the gate into a probe and the no-ghostty refusal becomes
    /// a process spawn on every `inspector.open` — on a host that has no `osascript` at all.
    #[tokio::test]
    async fn available_never_touches_the_runner() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(RecordingRunner::default());
        let plugin = GhosttyInspectorPlugin::new()
            .with_macos(true)
            .with_runner(Arc::clone(&runner) as Arc<dyn GhosttyRunner>);
        assert!(plugin.available(&context(dir.path())).await);
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    /// T-GHOST-3 — pi `owns: () => false` (`:14`).
    ///
    /// GUT it to `true` and ghostty becomes the owner the dispatcher finds for `inspector.status`,
    /// so a status query answers *"Inspector plugin 'ghostty' does not support status…"* instead
    /// of upstream's non-error *"No inspector plugin owns this binding…"* — and the herdr binding
    /// that actually exists is never consulted, because `find` stops at the first owner.
    #[tokio::test]
    async fn ghostty_owns_nothing_and_supplies_neither_optional_method() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = GhosttyInspectorPlugin::new().with_macos(true);
        let ctx = context(dir.path());
        assert!(!plugin.owns(&ctx));
        assert!(plugin.status(&ctx).await.is_none());
        assert!(plugin.close(&ctx).await.is_none());
    }

    /// The plugin's `open` is a pass-through to [`super::actions::open_ghostty_inspector`] over
    /// the injected seam. GUT the seam selection to always build [`OsascriptRunner`] and every
    /// ghostty test on a non-macOS box starts spawning a binary that is not there.
    #[tokio::test]
    async fn open_runs_the_injected_seam() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(RecordingRunner::default());
        let plugin = GhosttyInspectorPlugin::new()
            .with_macos(true)
            .with_runner(Arc::clone(&runner) as Arc<dyn GhosttyRunner>);
        let launch = InspectorLaunch {
            exe: "cyrup".to_owned(),
            args: Vec::new(),
            display_command: "cyrup __subagent-inspector".to_owned(),
        };
        plugin
            .open(
                &context(dir.path()),
                &launch,
                &InspectorParams {
                    pane_id: None,
                    focus: Some(true),
                },
            )
            .await
            .unwrap();
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0][0], "-e");
        assert_eq!(calls[0][3], "cyrup __subagent-inspector");
        assert_eq!(calls[0][5], "true");
    }

    #[test]
    fn the_plugin_name_is_upstreams() {
        assert_eq!(GhosttyInspectorPlugin::new().name(), "ghostty");
    }
}
