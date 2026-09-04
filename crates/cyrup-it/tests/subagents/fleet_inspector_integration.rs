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

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Mutex;

use cyrup_ext::HostServices;
use cyrup_ext::event::HostEvent;
use cyrup_ext::host::{WidgetEffect, WidgetPlacement};
use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::tui::fleet_status::FLEET_STATUS_WIDGET_KEY;

/// A `HostServices` backend that records every `set_widget` call the extension publishes.
///
/// EXT-047 re-signed `HostServices::set_widget` from one opaque `&Value` to pi's three arguments
/// `(key, content, options)` (`extensions/types.ts:170-175` @v0.83.0), and that re-signing is a
/// member of the `cyrup:ext@0.5` → `@0.6` world bump (`manifest.rs:169-176`). Recording a
/// [`WidgetEffect`] rather than a hand-rolled JSON blob is the point of the change: the removal
/// case is `lines: None`, not `{"content": null}`.
#[derive(Default)]
struct RecordingWidgetServices {
    widgets: Mutex<Vec<WidgetEffect>>,
}

impl HostServices for RecordingWidgetServices {
    fn set_widget(&self, key: &str, lines: Option<&[String]>, placement: WidgetPlacement) {
        self.widgets
            .lock()
            .expect("widget lock")
            .push(WidgetEffect {
                key: key.to_string(),
                lines: lines.map(<[String]>::to_vec),
                placement,
            });
    }
    fn session_id(&self) -> Option<String> {
        Some(TEST_SESSION_ID.to_string())
    }
}

/// The one session id every host backend in this file reports, and the one the on-disk fixtures
/// below stamp onto their `status.json` — because `listAsyncRuns`' session filter
/// (`async-status.ts:432`) compares exactly those two.
const TEST_SESSION_ID: &str = "fleet-session";

/// As [`RecordingWidgetServices`], but for the overlay backend below.
///
/// A `HostServices` backend that DRIVES an interactive overlay the way `cyrup-tui`'s run loop does:
/// paints a frame, feeds a scripted key sequence, paints again, and tears the modal down.
///
/// This is the seam `showFleet`'s third outcome depends on (pi `ctx.ui.custom(factory,
/// { overlay: true, … })`, `tui/fleet.ts:869-875`). Asserting on the frames it captures proves the
/// component is genuinely hosted — rendered AND driven — rather than constructed and dropped.
#[derive(Default)]
struct OverlayHostServices {
    widgets: Mutex<Vec<WidgetEffect>>,
    /// Keys fed to the overlay between the first and last captured frame.
    script: Mutex<Vec<cyrup_ext::OverlayKey>>,
    /// Every frame painted, flattened to text, in paint order.
    frames: Mutex<Vec<String>>,
    /// Every outcome the overlay returned for a scripted key, in order.
    outcomes: Mutex<Vec<cyrup_ext::OverlayOutcome>>,
    /// The rows/columns the host reports on each paint.
    width: usize,
    rows: usize,
}

impl OverlayHostServices {
    fn new(width: usize, rows: usize, script: Vec<cyrup_ext::OverlayKey>) -> Self {
        Self {
            widgets: Mutex::new(Vec::new()),
            script: Mutex::new(script),
            frames: Mutex::new(Vec::new()),
            outcomes: Mutex::new(Vec::new()),
            width,
            rows,
        }
    }

    fn frames(&self) -> Vec<String> {
        self.frames.lock().expect("frames lock").clone()
    }

    fn first_frame(&self) -> String {
        self.frames().first().cloned().unwrap_or_default()
    }

    fn outcomes(&self) -> Vec<cyrup_ext::OverlayOutcome> {
        self.outcomes.lock().expect("outcomes lock").clone()
    }
}

fn flatten(lines: &[cyrup_ext::OverlayLine]) -> String {
    lines
        .iter()
        .map(cyrup_ext::OverlayLine::plain_text)
        .collect::<Vec<_>>()
        .join("\n")
}

impl HostServices for OverlayHostServices {
    fn set_widget(&self, key: &str, lines: Option<&[String]>, placement: WidgetPlacement) {
        self.widgets
            .lock()
            .expect("widget lock")
            .push(WidgetEffect {
                key: key.to_string(),
                lines: lines.map(<[String]>::to_vec),
                placement,
            });
    }
    fn session_id(&self) -> Option<String> {
        Some(TEST_SESSION_ID.to_string())
    }
    fn open_overlay(&self, mut overlay: Box<dyn cyrup_ext::InteractiveOverlay>) -> bool {
        let first = overlay.render(self.width, self.rows);
        self.frames
            .lock()
            .expect("frames lock")
            .push(flatten(&first));
        let script = std::mem::take(&mut *self.script.lock().expect("script lock"));
        for key in script {
            let outcome = overlay.handle_key(key);
            self.outcomes.lock().expect("outcomes lock").push(outcome);
            let frame = overlay.render(self.width, self.rows);
            self.frames
                .lock()
                .expect("frames lock")
                .push(flatten(&frame));
        }
        true
    }
}

/// Write the exact `status.json` record the detached hop-2 runner writes, stamped with
/// `session_id` — the field `collect_fleet_history`'s session filter reads.
fn write_status_json(
    async_root: &std::path::Path,
    run_token: &str,
    agent: &str,
    session_id: Option<&str>,
) {
    let run_dir = async_root.join(run_token);
    std::fs::create_dir_all(&run_dir).expect("run dir");
    let mut status = cyrup_ext_subagents::background::RunStatus::queued(
        cyrup_ext_subagents::background::RunId::from_token(run_token.to_string()),
        cyrup_ext_subagents::background::RunMode::Single,
        None,
    );
    status.state = cyrup_ext_subagents::background::RunState::Running;
    status.session_id = session_id.map(str::to_string);
    status.steps = vec![cyrup_ext_subagents::background::StepStatus::pending(
        agent.to_string(),
    )];
    std::fs::write(
        run_dir.join("status.json"),
        serde_json::to_vec(&status).expect("serialize status"),
    )
    .expect("write status.json");
}

fn extension(cwd: &std::path::Path, home: &std::path::Path) -> SubagentsExtension {
    extension_with(cwd, home, SubagentExtensionConfig::default())
}

fn extension_with(
    cwd: &std::path::Path,
    home: &std::path::Path,
    mut config: SubagentExtensionConfig,
) -> SubagentsExtension {
    config.roots = Roots::sandboxed(home);
    SubagentsExtension::with_config_and_cwd(config, cwd.to_path_buf())
}

/// A hermetic root for one test, handed to the extension as `SubagentExtensionConfig::roots`
/// and to `run_artifact_roots_in` directly. No lock and no `set_var`: this run names its own
/// root instead of moving state every other test in this binary shares.
fn sandbox_home() -> tempfile::TempDir {
    tempfile::tempdir().expect("home tempdir")
}

// =================================================================================================
// (1) `/subagents-fleet` → showFleet
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagents_fleet_without_a_ui_renders_upstreams_text_fallback() {
    let home = sandbox_home();
    let dir = tempfile::tempdir().expect("tempdir");
    let ext = extension(dir.path(), home.path());
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
    let home = sandbox_home();
    let dir = tempfile::tempdir().expect("tempdir");
    let ext = extension(dir.path(), home.path());
    let host = std::sync::Arc::new(OverlayHostServices::new(100, 32, Vec::new()));
    ext.executor().set_host_services(host.clone());
    let ctx = HostCtx::command(ExtMode::Tui, true, dir.path().to_path_buf());

    let out = ext
        .execute_command("subagents-fleet", "", &ctx)
        .await
        .expect("command dispatched")
        .expect("command produced output");

    // pi's `ctx.ui.custom<undefined>` resolves with no value; the modal already said everything on
    // screen, so the command itself returns nothing to surface as a notification.
    assert_eq!(
        out, "",
        "a hosted overlay must not ALSO return text, got:\n{out}"
    );

    // The frame, its title, its empty-roster state and its footer — all from
    // `SubagentFleetComponent::render` (pi `tui/fleet.ts:788-830`), now painted BY THE HOST.
    let frame = host.first_frame();
    assert!(frame.contains("Subagent fleet inspector"), "got:\n{frame}");
    assert!(frame.contains("· live controls"), "got:\n{frame}");
    assert!(frame.contains("No tracked children"), "got:\n{frame}");
    assert!(
        frame.contains("↑↓/jk agent · H Herdr · s steer · D stop"),
        "got:\n{frame}"
    );
    assert!(frame.contains('╭') && frame.contains('╰'), "got:\n{frame}");
}

/// The half that did not exist before the overlay seam: the component is DRIVEN, not just painted.
/// `set_terminal_rows` is what sizes the body, and it had no production caller at all — so the
/// inspector rendered at its `32`-row default no matter how tall the terminal was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hosted_inspector_sizes_its_body_from_the_hosts_reported_rows() {
    let home = sandbox_home();
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path();
    let roots =
        cyrup_ext_subagents::background::run_artifact_roots_in(&Roots::sandboxed(home.path()), cwd);
    write_status_json(
        &roots.async_root,
        "fleetrun001",
        "historian",
        Some(TEST_SESSION_ID),
    );

    let mut heights = Vec::new();
    for rows in [24usize, 60usize] {
        let ext = extension(cwd, home.path());
        let host = std::sync::Arc::new(OverlayHostServices::new(100, rows, Vec::new()));
        ext.executor().set_host_services(host.clone());
        let ctx = HostCtx::command(ExtMode::Tui, true, cwd.to_path_buf());
        ext.execute_command("subagents-fleet", "", &ctx)
            .await
            .expect("command dispatched");
        heights.push(host.first_frame().lines().count());
    }
    // pi `bodyHeight = max(2, floor(rows * 0.85) - 6)` + 6 chrome rows (`fleet.ts:792`), i.e. the
    // whole frame is `floor(rows * 0.85)` tall.
    assert_eq!(
        heights,
        vec![24 * 85 / 100, 60 * 85 / 100],
        "frames: {heights:?}"
    );
}

/// Keystrokes reach `handle_input` through the host, and `Esc` closes — the whole interactive half
/// that used to have no caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keystrokes_move_the_selection_and_escape_closes_the_hosted_inspector() {
    let home = sandbox_home();
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path();
    let roots =
        cyrup_ext_subagents::background::run_artifact_roots_in(&Roots::sandboxed(home.path()), cwd);
    write_status_json(
        &roots.async_root,
        "fleetrun001",
        "historian",
        Some(TEST_SESSION_ID),
    );
    write_status_json(
        &roots.async_root,
        "fleetrun002",
        "archivist",
        Some(TEST_SESSION_ID),
    );

    use cyrup_ext::{OverlayKey, OverlayKeyCode, OverlayOutcome};
    let ext = extension(cwd, home.path());
    let host = std::sync::Arc::new(OverlayHostServices::new(
        100,
        32,
        vec![
            OverlayKey::plain(OverlayKeyCode::Down),
            OverlayKey::plain(OverlayKeyCode::Char('x')),
            OverlayKey::plain(OverlayKeyCode::Escape),
        ],
    ));
    ext.executor().set_host_services(host.clone());
    let ctx = HostCtx::command(ExtMode::Tui, true, cwd.to_path_buf());
    ext.execute_command("subagents-fleet", "", &ctx)
        .await
        .expect("command dispatched");

    assert_eq!(
        host.outcomes(),
        vec![
            OverlayOutcome::Redraw,
            OverlayOutcome::Redraw,
            OverlayOutcome::Close
        ],
        "Down and x redraw; Esc closes (pi `fleet.ts:660-663,666-667,708-712`)"
    );
    let frames = host.frames();
    assert_eq!(
        frames.len(),
        4,
        "one frame before the script and one after each key"
    );
    // Both runs are on the roster, and the selection cursor moved from the first to the second.
    assert!(frames[0].contains("historian"), "got:\n{}", frames[0]);
    assert!(frames[0].contains("archivist"), "got:\n{}", frames[0]);
    assert!(
        frames[0].contains("1/2"),
        "the position readout starts at 1/2:\n{}",
        frames[0]
    );
    assert!(
        frames[1].contains("2/2"),
        "Down must move the selection:\n{}",
        frames[1]
    );
    assert_ne!(
        frames[0], frames[1],
        "a moved selection must change the painted frame"
    );
}

/// The command's registered description must be v0.43.0's, since its handler is now v0.43.0's.
#[test]
fn the_registered_description_is_the_v0_43_0_text() {
    let descriptor = cyrup_ext_subagents::registration::slash_commands::SLASH_COMMANDS
        .iter()
        .find(|d| d.name.as_str() == "subagents-fleet")
        .expect("subagents-fleet is registered");
    assert_eq!(
        descriptor.description,
        "Open the live subagent fleet inspector"
    );
}

// =================================================================================================
// (2) The always-on fleet status widget
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fleet_status_widget_is_published_and_cleared_through_live_host_services() {
    let home = sandbox_home();
    let dir = tempfile::tempdir().expect("tempdir");
    let ext = extension(dir.path(), home.path());
    let services = std::sync::Arc::new(RecordingWidgetServices::default());
    ext.set_host_services(services.clone());

    let ctx = HostCtx::event(ExtMode::Tui, true, dir.path().to_path_buf());

    // An `AgentEnd` edge with NO active subagents publishes nothing (pi's `entries.length === 0`
    // early return, `tui/fleet-status.ts:315-325`, which never registers the widget in the first
    // place).
    ext.on_event(
        &HostEvent::AgentEnd {
            messages: Vec::new(),
        },
        &ctx,
    )
    .await;
    assert!(
        services.widgets.lock().expect("lock").is_empty(),
        "an idle fleet must not publish a widget"
    );

    // Shutdown always clears the key, so the host can never be left holding a stale widget.
    ext.on_event(
        &HostEvent::SessionShutdown {
            reason: "test".into(),
            target_session_file: None,
        },
        &ctx,
    )
    .await;
    let widgets = services.widgets.lock().expect("lock").clone();
    let clear = widgets.last().expect("shutdown publishes a clear");
    assert_eq!(clear.key, FLEET_STATUS_WIDGET_KEY);
    // EXT-047: the removal is pi's `setWidget(key, undefined)` — an absent `content` ARGUMENT
    // (`tui/fleet-status.ts:309,320`) — not a `{"content": null}` blob. `lines: None` is the only
    // shape that removes the key; anything else leaves the slot occupied.
    assert_eq!(clear.lines, None, "shutdown REMOVES the widget: {clear:?}");
}

// =================================================================================================
// (3) The on-disk history scan
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_status_json_under_the_async_root_becomes_a_roster_row() {
    let home = sandbox_home();
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path();

    // Write the exact record the detached hop-2 runner writes: `<async_root>/<run-id>/status.json`,
    // stamped with THIS session — pi's `listAsyncRuns({ sessionId })` filter (`async-status.ts:432`)
    // compares that field against the caller's session and drops everything else.
    let roots =
        cyrup_ext_subagents::background::run_artifact_roots_in(&Roots::sandboxed(home.path()), cwd);
    write_status_json(
        &roots.async_root,
        "fleetrun001",
        "historian",
        Some(TEST_SESSION_ID),
    );

    let ext = extension(cwd, home.path());
    let host = std::sync::Arc::new(OverlayHostServices::new(100, 32, Vec::new()));
    ext.executor().set_host_services(host.clone());
    let ctx = HostCtx::command(ExtMode::Tui, true, cwd.to_path_buf());
    ext.execute_command("subagents-fleet", "", &ctx)
        .await
        .expect("command dispatched");

    let out = host.first_frame();
    assert!(
        out.contains("historian"),
        "the roster must list the on-disk run, got:\n{out}"
    );
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

/// pi `listAsyncRuns`' session filter (`async-status.ts:432`), end to end against real files:
/// `if (options.sessionId && status.sessionId !== options.sessionId) continue;`.
///
/// Note the exact shape — a run with NO recorded session is dropped too, because
/// `undefined !== "fleet-session"`. Without this filter, opening the inspector listed every run
/// every session in the project had ever launched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_history_scan_keeps_only_this_sessions_runs() {
    let home = sandbox_home();
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path();
    let roots =
        cyrup_ext_subagents::background::run_artifact_roots_in(&Roots::sandboxed(home.path()), cwd);
    write_status_json(
        &roots.async_root,
        "runmine0001",
        "mine",
        Some(TEST_SESSION_ID),
    );
    write_status_json(
        &roots.async_root,
        "runtheirs01",
        "theirs",
        Some("another-session"),
    );
    write_status_json(&roots.async_root, "runnosess01", "untagged", None);

    let ext = extension(cwd, home.path());
    let host = std::sync::Arc::new(OverlayHostServices::new(100, 32, Vec::new()));
    ext.executor().set_host_services(host.clone());
    let ctx = HostCtx::command(ExtMode::Tui, true, cwd.to_path_buf());
    ext.execute_command("subagents-fleet", "", &ctx)
        .await
        .expect("command dispatched");

    let out = host.first_frame();
    assert!(
        out.contains("mine"),
        "this session's run must be listed, got:\n{out}"
    );
    assert!(
        !out.contains("theirs"),
        "another session's run must be dropped, got:\n{out}"
    );
    assert!(
        !out.contains("untagged"),
        "an untagged run loses to a present filter (undefined !== id), got:\n{out}"
    );
    assert!(
        out.contains("1/1"),
        "exactly one roster row survives, got:\n{out}"
    );
}

// =================================================================================================
// (4) The two FleetView config keys (pi `extension/index.ts:333-334,378-383`)
// =================================================================================================

/// pi `config.fleetView !== false` — an explicit `false` means no `SubagentFleetStatus` at all, so
/// not even the shutdown clear reaches the host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fleet_view_false_publishes_no_widget_at_all() {
    let home = sandbox_home();
    let dir = tempfile::tempdir().expect("tempdir");
    let config = SubagentExtensionConfig {
        fleet_view: false,
        ..SubagentExtensionConfig::default()
    };
    let ext = extension_with(dir.path(), home.path(), config);
    let services = std::sync::Arc::new(RecordingWidgetServices::default());
    ext.set_host_services(services.clone());
    let ctx = HostCtx::event(ExtMode::Tui, true, dir.path().to_path_buf());

    ext.on_event(
        &HostEvent::SessionStart {
            reason: "test".into(),
            previous_session_file: None,
        },
        &ctx,
    )
    .await;
    ext.on_event(
        &HostEvent::AgentEnd {
            messages: Vec::new(),
        },
        &ctx,
    )
    .await;
    assert!(
        services.widgets.lock().expect("lock").is_empty(),
        "fleetView:false must publish nothing"
    );
}

/// pi `resolveFleetViewPlacement(config.fleetViewPlacement)` — only the exact string moves it up.
#[test]
fn fleet_view_placement_is_resolved_from_config() {
    use cyrup_ext_subagents::tui::fleet_status::{
        FleetViewPlacement, resolve_fleet_view_placement,
    };
    let config = SubagentExtensionConfig {
        fleet_view_placement: Some("aboveEditor".to_string()),
        ..SubagentExtensionConfig::default()
    };
    assert_eq!(
        resolve_fleet_view_placement(config.fleet_view_placement.as_deref()),
        FleetViewPlacement::AboveEditor
    );
    assert_eq!(
        resolve_fleet_view_placement(
            SubagentExtensionConfig::default()
                .fleet_view_placement
                .as_deref()
        ),
        FleetViewPlacement::BelowEditor
    );
}
