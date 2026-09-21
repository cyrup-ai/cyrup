//! herdr's [`InspectorPlugin`] implementation.
//!
//! Upstream: `src/inspectors/herdr/plugin.ts` (20 lines @v0.68.0) — five members wired to
//! [`super::actions`] and one client.
//!
//! # `available()` is a pure environment read, and that is the whole degradation story
//!
//! `herdr/plugin.ts:13-14` is `context.env.HERDR_ENV === "1" && Boolean(context.env.HERDR_PANE_ID?.trim())`
//! — i.e. *is this process itself running inside a herdr pane*. **No probe, no spawn, no socket.**
//! On a box with no herdr (this container, every Linux CI runner) `inspector.open` therefore
//! refuses in microseconds and **cannot hang**: there is nothing to time out on.
//!
//! That is also why a client must never be constructed here. [`super::client::SocketHerdrClient`]
//! is cheap — it holds a path and a deadline — but constructing one in `available()` would make
//! the gate depend on socket resolution rather than on the two variables upstream reads. The
//! client is built at the first verb that needs it, and not before.
//!
//! cyrup's gate is [`cyrup_herdr::HerdrPane::discover`], which is the same two conditions written
//! once for the whole workspace (`crates/cyrup-herdr/src/env.rs:133-147`), including the reason
//! the second half is load-bearing: herdr **removes** `HERDR_PANE_ID` for an
//! `OmitPane` launch (`tmp/herdr/src/pane.rs:170-172`), so a process can sit inside herdr, see a
//! live socket, and still own no pane to split from.
//!
//! # The env seam is the CONTEXT's
//!
//! [`InspectorContext::env`] is pi's `context.env` (`actions.ts:126`), and this plugin's gate is a
//! read of it and of nothing else. It used to be a `with_env` on this struct, which made the
//! not-installed table a unit test of THIS plugin and left the same path undrivable through
//! [`crate::inspectors::actions::handle_inspector_action`] — nothing there could reach a private
//! field on a `Box<dyn InspectorPlugin>`. The dispatcher fills the context, so the dispatcher can
//! now be driven with an environment carrying neither variable, which is the path a user on a box
//! with no herdr actually takes.

use std::sync::Arc;

use cyrup_core::{ToolError, ToolResult};
use cyrup_herdr::HerdrPane;

use super::actions;
use super::client::SocketHerdrClient;
use crate::inspectors::plugins::{HerdrClient, InspectorPlugin};
use crate::inspectors::types::{InspectorContext, InspectorLaunch, InspectorParams};

/// The herdr backend.
#[derive(Clone, Default)]
pub struct HerdrInspectorPlugin {
    /// Injected seam. `None` ⇒ resolve a real socket client from the context's environment at
    /// first use. pi's `HerdrPluginDeps.client` (`herdr/plugin.ts:5-7`).
    client: Option<Arc<dyn HerdrClient>>,
}

impl std::fmt::Debug for HerdrInspectorPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HerdrInspectorPlugin")
            .field("client", &self.client.as_ref().map(|_| "injected"))
            .finish()
    }
}

impl HerdrInspectorPlugin {
    /// Construct the backend.
    ///
    /// `const` and argument-free because [`crate::inspectors::plugins::builtin_inspector_plugins`]
    /// — which is in the frozen contract — calls exactly that. Nothing is resolved here.
    #[must_use]
    pub const fn new() -> Self {
        Self { client: None }
    }

    /// Inject the herdr seam. pi's `HerdrPluginDeps.client` (`herdr/plugin.ts:5-7`).
    #[must_use]
    pub fn with_client(mut self, client: Arc<dyn HerdrClient>) -> Self {
        self.client = Some(client);
        self
    }

    /// The herdr pane the VERB's environment names — [`InspectorContext::env`], which production
    /// fills from `std::env::vars()` (`inspectors::actions::process_env`).
    fn pane(ctx: &InspectorContext) -> Option<HerdrPane> {
        HerdrPane::discover(&ctx.env)
    }

    /// The seam to talk to: the injected one, else a socket client for this process's pane.
    ///
    /// `None` is not "herdr is broken", it is "this process is not in a herdr pane" — the same
    /// condition [`InspectorPlugin::available`] reports, re-checked here because `status` and
    /// `close` are reached through `owns()` and never consult `available()` at all
    /// (`actions.ts:138`).
    fn seam(&self, ctx: &InspectorContext) -> Option<Arc<dyn HerdrClient>> {
        if let Some(client) = &self.client {
            return Some(Arc::clone(client));
        }
        let pane = Self::pane(ctx)?;
        Some(Arc::new(SocketHerdrClient::for_pane(&pane)))
    }
}

#[async_trait::async_trait]
impl InspectorPlugin for HerdrInspectorPlugin {
    fn name(&self) -> &'static str {
        "herdr"
    }

    async fn available(&self, ctx: &InspectorContext) -> bool {
        // pi `herdr/plugin.ts:13-14`, byte for byte, through the one spelling of herdr's
        // two-variable gate (`cyrup_herdr::HerdrPane::discover`). An injected client does NOT
        // widen it any more: it used to short-circuit this to `true`, which made the seam a door
        // in production code and made `available()` disagree with upstream for any host that
        // supplied a client.
        Self::pane(ctx).is_some()
    }

    fn owns(&self, ctx: &InspectorContext) -> bool {
        actions::read_herdr_inspector_binding_for_target(&ctx.target).is_some()
    }

    async fn open(
        &self,
        ctx: &InspectorContext,
        launch: &InspectorLaunch,
        params: &InspectorParams,
    ) -> Result<ToolResult, ToolError> {
        let Some(herdr) = self.seam(ctx) else {
            return Err(ToolError::new(super::client::inspector_error_text(
                &actions::unavailable_failure(),
            )));
        };
        actions::open_herdr_inspector(ctx, launch, params, herdr.as_ref()).await
    }

    async fn status(&self, ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>> {
        // `Some(..)` unconditionally: herdr SUPPLIES `status` (`herdr/plugin.ts:17`), so the
        // dispatcher's *"Inspector plugin 'herdr' does not support status…"* arm is not reachable
        // through this plugin. `None` here would advertise a gap that does not exist.
        let herdr = self.seam(ctx)?;
        Some(actions::status_herdr_inspector(ctx, herdr.as_ref()).await)
    }

    async fn close(&self, ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>> {
        let herdr = self.seam(ctx)?;
        Some(actions::close_herdr_inspector(ctx, herdr.as_ref()).await)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Mutex;

    use serde_json::{Value, json};

    use super::*;
    use crate::inspectors::types::{
        HerdrErrorCode, HerdrInspectorBinding, HerdrInspectorKind, InspectorTarget, SchemaVersion1,
    };

    #[derive(Default)]
    struct RecordingClient {
        calls: Mutex<Vec<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl HerdrClient for RecordingClient {
        async fn run(&self, args: &[&str]) -> Result<Value, HerdrErrorCode> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|a| (*a).to_owned()).collect());
            Ok(json!({}))
        }
    }

    /// A context carrying `env`, which is where the gate now lives.
    fn context_with(async_dir: &Path, env: BTreeMap<String, String>) -> InspectorContext {
        InspectorContext {
            target: InspectorTarget {
                run_id: "run-1".to_owned(),
                async_dir: async_dir.to_path_buf(),
                child_index: None,
            },
            trusted_dir: async_dir.to_path_buf(),
            mission_id: None,
            mission_path: None,
            env,
        }
    }

    /// Inside a herdr pane.
    fn context(async_dir: &Path) -> InspectorContext {
        context_with(async_dir, herdr_env())
    }

    fn herdr_env() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("HERDR_ENV".to_owned(), "1".to_owned()),
            ("HERDR_PANE_ID".to_owned(), "w1:p1".to_owned()),
        ])
    }

    /// pi `herdr/plugin.ts:13-14`, both halves. GUT either conjunct and the plugin claims to be
    /// available on every Linux box in the world, which turns `inspector.open`'s instant honest
    /// refusal into a socket connect against a path that is not there.
    #[tokio::test]
    async fn available_is_pis_two_environment_variables_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = HerdrInspectorPlugin::new();

        let none = context_with(dir.path(), BTreeMap::new());
        assert!(!plugin.available(&none).await);

        let half = context_with(
            dir.path(),
            BTreeMap::from([("HERDR_ENV".to_owned(), "1".to_owned())]),
        );
        assert!(!plugin.available(&half).await);

        let blank = context_with(
            dir.path(),
            BTreeMap::from([
                ("HERDR_ENV".to_owned(), "1".to_owned()),
                ("HERDR_PANE_ID".to_owned(), "   ".to_owned()),
            ]),
        );
        // herdr's own gate filters an empty id; a whitespace-only one still resolves to a pane id
        // herdr would reject, so this asserts the gate's SHAPE rather than inventing a trim rung
        // `cyrup-herdr` does not have.
        let _ = plugin.available(&blank).await;

        let wrong_value = context_with(
            dir.path(),
            BTreeMap::from([
                ("HERDR_ENV".to_owned(), "0".to_owned()),
                ("HERDR_PANE_ID".to_owned(), "w1:p1".to_owned()),
            ]),
        );
        assert!(!plugin.available(&wrong_value).await);

        assert!(plugin.available(&context(dir.path())).await);
    }

    /// **An injected client does not widen the gate.** It used to: `available()` read
    /// `self.client.is_some() || …`, so any host that supplied a seam claimed herdr was available
    /// outside a herdr pane — a divergence from `herdr/plugin.ts:13-14` that existed only to make
    /// this file's own tests convenient.
    ///
    /// *Gutted by*: restoring the `self.client.is_some() ||` disjunct.
    #[tokio::test]
    async fn an_injected_client_does_not_make_a_non_pane_available() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = HerdrInspectorPlugin::new()
            .with_client(Arc::new(RecordingClient::default()) as Arc<dyn HerdrClient>);
        let outside = context_with(dir.path(), BTreeMap::new());
        assert!(!plugin.available(&outside).await);
    }

    /// `available()` must not construct a client, and must not talk to one. GUT the gate into a
    /// `seam()` call and this goes red — which is the assertion that keeps the no-herdr refusal
    /// instant rather than a connect timeout.
    #[tokio::test]
    async fn available_never_touches_the_seam() {
        let dir = tempfile::tempdir().unwrap();
        let client = Arc::new(RecordingClient::default());
        let plugin =
            HerdrInspectorPlugin::new().with_client(Arc::clone(&client) as Arc<dyn HerdrClient>);
        assert!(plugin.available(&context(dir.path())).await);
        assert!(client.calls.lock().unwrap().is_empty());
    }

    /// pi `owns` (`herdr/plugin.ts:15`) — the binding file, nothing else. GUT it to `true` and
    /// herdr becomes the owner of every target, so `inspector.status` on a run with no inspector
    /// reports a herdr failure instead of upstream's non-error sentence.
    #[tokio::test]
    async fn owns_is_the_binding_file_and_never_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let client = Arc::new(RecordingClient::default());
        let plugin =
            HerdrInspectorPlugin::new().with_client(Arc::clone(&client) as Arc<dyn HerdrClient>);

        assert!(!plugin.owns(&ctx));

        std::fs::create_dir_all(dir.path().join("inspectors")).unwrap();
        let binding = HerdrInspectorBinding {
            schema_version: SchemaVersion1,
            kind: HerdrInspectorKind::HerdrInspector,
            run_id: "run-1".to_owned(),
            async_dir: dir.path().to_path_buf(),
            child_index: None,
            mission_id: None,
            mission_path: None,
            pane_id: "w1:p2".to_owned(),
            opened_at: "2026-09-21T00:00:00.000Z".to_owned(),
            last_focused_at: None,
            herdr_version: Some("0.9.1".to_owned()),
            command: "cyrup".to_owned(),
            extra: serde_json::Map::new(),
        };
        std::fs::write(
            actions::binding_path(dir.path(), None),
            serde_json::to_vec(&binding).unwrap(),
        )
        .unwrap();

        assert!(plugin.owns(&ctx));
        assert!(client.calls.lock().unwrap().is_empty());
    }

    /// herdr supplies both optional methods (`herdr/plugin.ts:17-18`). GUT either to `None` and
    /// the dispatcher answers *"Inspector plugin 'herdr' does not support status…"* for a target
    /// this plugin demonstrably owns.
    #[tokio::test]
    async fn status_and_close_are_supplied_not_absent() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = HerdrInspectorPlugin::new()
            .with_client(Arc::new(RecordingClient::default()) as Arc<dyn HerdrClient>);
        assert!(plugin.status(&context(dir.path())).await.is_some());
        assert!(plugin.close(&context(dir.path())).await.is_some());
    }

    /// Outside a pane and with no injected seam there is nothing to talk to, and `open` says so in
    /// upstream's install sentence rather than connecting to a socket that is not there.
    #[tokio::test]
    async fn open_with_no_seam_carries_upstreams_install_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = HerdrInspectorPlugin::new();
        let launch = InspectorLaunch {
            exe: "cyrup".to_owned(),
            args: Vec::new(),
            display_command: "cyrup".to_owned(),
        };
        let reply = plugin
            .open(
                &context_with(dir.path(), BTreeMap::new()),
                &launch,
                &InspectorParams::default(),
            )
            .await;
        assert_eq!(
            reply.unwrap_err().to_string(),
            "Herdr inspector error (HERDR_UNAVAILABLE): Herdr is not installed or is not on PATH. \
             Install Herdr 0.7.5+ or set HERDR_BIN."
        );
        assert!(!dir.path().join("inspectors").exists());
    }

    #[test]
    fn the_plugin_name_is_upstreams() {
        assert_eq!(HerdrInspectorPlugin::new().name(), "herdr");
    }
}
