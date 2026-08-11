//! Integration test: the FleetView port (`pi-subagents/src/tui/fleet.ts`, `fleet-status.ts`,
//! `fleet-transcript.ts` @v0.43.0) is reachable from the REAL production surfaces, not only from
//! its own unit tests.
//!
//! Two seams are proved here, each end to end through the `cyrup_ext::native::NativeExtension`
//! trait — the same objects the `cyrup` binary drives, no mocking of the extension itself:
//!
//! 1. **`/subagents-fleet` → `showFleet(ctx)`** (`slash/slash-commands.ts:714-717` @v0.43.0).
//!    `NativeExtension::execute_command("subagents-fleet", …)` with a `has_ui: false` `HostCtx`
//!    must take upstream's own no-UI fallback (`:635-638`) and render the v0.34.0 text fleet view;
//!    with `has_ui: true` it must construct and render the interactive inspector
//!    (`crate::tui::fleet::SubagentFleetComponent`).
//! 2. **The always-on fleet status widget** (`tui/fleet-status.ts:301-350`). A `SessionStart` /
//!    `AgentEnd` / `SessionShutdown` event dispatched at `NativeExtension::on_event` must publish —
//!    and finally clear — the widget through the live `HostServices::set_widget`, under the
//!    `subagent-fleet-status` key upstream registers it with.
//! 3. **The on-disk history scan** (`fleet.ts:194-199`'s `listAsyncRuns(asyncDirRoot, …,
//!    reconcile: false)`). A REAL `status.json` written under the run's REAL async root must show
//!    up as a roster row in the rendered inspector — no fixture, no mock, just the file the
//!    detached runner writes.
//!
//! Every test here pins `$CYRUP_HOME` at a hermetic tempdir, because the async/results roots
//! (`background::run_artifact_roots`) are derived from it — without that a developer's real
//! background runs would leak into these assertions.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::sync::Mutex;

use cyrup_ext::HostServices;
use cyrup_ext::event::HostEvent;
use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::tui::fleet_status::FLEET_STATUS_WIDGET_KEY;

/// A `HostServices` backend that records every `set_widget` payload the extension publishes.
#[derive(Default)]
struct RecordingWidgetServices {
    widgets: Mutex<Vec<serde_json::Value>>,
}

impl HostServices for RecordingWidgetServices {
    fn set_widget(&self, widget: &serde_json::Value) {
        self.widgets.lock().expect("widget lock").push(widget.clone());
    }
    fn session_id(&self) -> Option<String> {
        Some("fleet-session".to_string())
    }
}

fn extension(cwd: &std::path::Path) -> SubagentsExtension {
    SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        cwd.to_path_buf(),
    )
}

fn extension_with(cwd: &std::path::Path, config: SubagentExtensionConfig) -> SubagentsExtension {
    SubagentsExtension::with_config_and_cwd(config, cwd.to_path_buf())
}

/// Serializes the `$CYRUP_HOME` mutation below; every test in this binary shares one process.
/// A `tokio::sync::Mutex` rather than a `std` one because the guard is deliberately held across
/// the `.await`s in each test body — that is the point of the lock — and a `std` guard held across
/// an await is a real hazard clippy is right to flag.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Point `background::run_artifact_roots` at a hermetic root for the duration of one test.
/// Returns the guard + the tempdir, both of which must stay alive for the test's body.
async fn hermetic_home() -> (tokio::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    let guard = ENV_LOCK.lock().await;
    let home = tempfile::tempdir().expect("home tempdir");
    // SAFETY: scoped, mutex-serialized env mutation (Rust 2024 requires `unsafe` for set_var).
    unsafe {
        std::env::set_var("CYRUP_HOME", home.path());
    }
    (guard, home)
}

// =================================================================================================
// (1) `/subagents-fleet` → showFleet
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagents_fleet_without_a_ui_renders_upstreams_text_fallback() {
    let (_env, _home) = hermetic_home().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let ext = extension(dir.path());
    let ctx = HostCtx::command(ExtMode::Print, false, dir.path().to_path_buf());

    let out = ext
        .execute_command("subagents-fleet", "", &ctx)
        .await
        .expect("command dispatched")
        .expect("command produced output");

    // pi `showFleet`'s `!ctx.hasUI` branch is `runSlashSubagent(pi, ctx, { action: "status",
    // view: "fleet" })` — `inspectSubagentFleet`'s own empty-fleet sentence.
    assert!(
        out.contains("No active subagent fleet."),
        "expected the text fleet view, got:\n{out}"
    );
    assert!(
        !out.contains("Subagent fleet inspector"),
        "the no-UI branch must not build the overlay, got:\n{out}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagents_fleet_with_a_ui_renders_the_interactive_inspector_frame() {
    let (_env, _home) = hermetic_home().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let ext = extension(dir.path());
    let ctx = HostCtx::command(ExtMode::Tui, true, dir.path().to_path_buf());

    let out = ext
        .execute_command("subagents-fleet", "", &ctx)
        .await
        .expect("command dispatched")
        .expect("command produced output");

    // The frame, its title, its empty-roster state and its footer — all from
    // `SubagentFleetComponent::render` (pi `tui/fleet.ts:788-830`).
    assert!(out.contains("Subagent fleet inspector"), "got:\n{out}");
    assert!(out.contains("· live controls"), "got:\n{out}");
    assert!(out.contains("No tracked children"), "got:\n{out}");
    assert!(
        out.contains("↑↓/jk agent · H Herdr · s steer · D stop"),
        "got:\n{out}"
    );
    assert!(out.contains('╭') && out.contains('╰'), "got:\n{out}");
}

/// The command's registered description must be v0.43.0's, since its handler is now v0.43.0's.
#[test]
fn the_registered_description_is_the_v0_43_0_text() {
    let descriptor = cyrup_ext_subagents::registration::slash_commands::SLASH_COMMANDS
        .iter()
        .find(|d| d.name.as_str() == "subagents-fleet")
        .expect("subagents-fleet is registered");
    assert_eq!(descriptor.description, "Open the live subagent fleet inspector");
}

// =================================================================================================
// (2) The always-on fleet status widget
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fleet_status_widget_is_published_and_cleared_through_live_host_services() {
    let (_env, _home) = hermetic_home().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let ext = extension(dir.path());
    let services = std::sync::Arc::new(RecordingWidgetServices::default());
    ext.set_host_services(services.clone());

    let ctx = HostCtx::event(ExtMode::Tui, true, dir.path().to_path_buf());

    // An `AgentEnd` edge with NO active subagents publishes nothing (pi's `entries.length === 0`
    // early return, `tui/fleet-status.ts:315-325`, which never registers the widget in the first
    // place).
    ext.on_event(&HostEvent::AgentEnd { messages: Vec::new() }, &ctx).await;
    assert!(
        services.widgets.lock().expect("lock").is_empty(),
        "an idle fleet must not publish a widget"
    );

    // Shutdown always clears the key, so the host can never be left holding a stale widget.
    ext.on_event(
        &HostEvent::SessionShutdown { reason: "test".into() },
        &ctx,
    )
    .await;
    let widgets = services.widgets.lock().expect("lock").clone();
    let clear = widgets
        .last()
        .expect("shutdown publishes a clear");
    assert_eq!(
        clear.get("key").and_then(serde_json::Value::as_str),
        Some(FLEET_STATUS_WIDGET_KEY)
    );
    assert_eq!(clear.get("content"), Some(&serde_json::Value::Null));
}

// =================================================================================================
// (3) The on-disk history scan
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_status_json_under_the_async_root_becomes_a_roster_row() {
    let (_env, _home) = hermetic_home().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path();

    // Write the exact record the detached hop-2 runner writes: `<async_root>/<run-id>/status.json`.
    let roots = cyrup_ext_subagents::background::run_artifact_roots(cwd);
    let run_dir = roots.async_root.join("fleetrun001");
    std::fs::create_dir_all(&run_dir).expect("run dir");
    let mut status = cyrup_ext_subagents::background::RunStatus::queued(
        cyrup_ext_subagents::background::RunId::from_token("fleetrun001".to_string()),
        cyrup_ext_subagents::background::RunMode::Single,
        None,
    );
    status.state = cyrup_ext_subagents::background::RunState::Running;
    status.steps = vec![cyrup_ext_subagents::background::StepStatus::pending(
        "historian".to_string(),
    )];
    std::fs::write(
        run_dir.join("status.json"),
        serde_json::to_vec(&status).expect("serialize status"),
    )
    .expect("write status.json");

    let ext = extension(cwd);
    let ctx = HostCtx::command(ExtMode::Tui, true, cwd.to_path_buf());
    let out = ext
        .execute_command("subagents-fleet", "", &ctx)
        .await
        .expect("command dispatched")
        .expect("command produced output");

    assert!(out.contains("historian"), "the roster must list the on-disk run, got:\n{out}");
    assert!(!out.contains("No tracked children"), "got:\n{out}");
    assert!(out.contains("1/1"), "got:\n{out}");

    // The same run is visible through the no-UI text fallback too (R-SA-130: one fleet, two
    // surfaces).
    let text_ctx = HostCtx::command(ExtMode::Print, false, cwd.to_path_buf());
    let text = ext
        .execute_command("subagents-fleet", "", &text_ctx)
        .await
        .expect("command dispatched")
        .expect("command produced output");
    assert!(text.contains("fleetrun001"), "got:\n{text}");
}

// =================================================================================================
// (4) The two FleetView config keys (pi `extension/index.ts:333-334,378-383`)
// =================================================================================================

/// pi `config.fleetView !== false` — an explicit `false` means no `SubagentFleetStatus` at all, so
/// not even the shutdown clear reaches the host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fleet_view_false_publishes_no_widget_at_all() {
    let (_env, _home) = hermetic_home().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let config = SubagentExtensionConfig { fleet_view: false, ..SubagentExtensionConfig::default() };
    let ext = extension_with(dir.path(), config);
    let services = std::sync::Arc::new(RecordingWidgetServices::default());
    ext.set_host_services(services.clone());
    let ctx = HostCtx::event(ExtMode::Tui, true, dir.path().to_path_buf());

    ext.on_event(&HostEvent::SessionStart { reason: "test".into() }, &ctx).await;
    ext.on_event(&HostEvent::AgentEnd { messages: Vec::new() }, &ctx).await;
    assert!(
        services.widgets.lock().expect("lock").is_empty(),
        "fleetView:false must publish nothing"
    );
}

/// pi `resolveFleetViewPlacement(config.fleetViewPlacement)` — only the exact string moves it up.
#[test]
fn fleet_view_placement_is_resolved_from_config() {
    use cyrup_ext_subagents::tui::fleet_status::{FleetViewPlacement, resolve_fleet_view_placement};
    let config = SubagentExtensionConfig {
        fleet_view_placement: Some("aboveEditor".to_string()),
        ..SubagentExtensionConfig::default()
    };
    assert_eq!(
        resolve_fleet_view_placement(config.fleet_view_placement.as_deref()),
        FleetViewPlacement::AboveEditor
    );
    assert_eq!(
        resolve_fleet_view_placement(SubagentExtensionConfig::default().fleet_view_placement.as_deref()),
        FleetViewPlacement::BelowEditor
    );
}
