//! FLUX-003 — the three surfaces that reach the host: the status overlay behind
//! `STATUS_OVERLAY_SHORTCUT`, the `ask_user_question` tool, and the native command routing in
//! `extension.rs`. None of these has an upstream script to diff against — the overlay is the
//! cyrup-native restoration of Wibey's `ui-mode: flux-status` panel (`flux_status.py:4-6`
//! @v0.0.40 explains why code-puppy prints a static one instead), the tool bridges code-puppy's
//! structured question onto `HostServices::select`, and the command handlers carry the Python
//! `main()` wordings (`flux_status.py:323-326`, `flux_cheatsheet.py:207-209`). What is pinned
//! is therefore the contract the crate's own docs state: the overlay draws the SAME text
//! `/flux/status` prints (`overlay.rs` header: "reuses … verbatim; only the OUTPUT shape
//! differs"), the tool's parameter validation, prompt shape, display-row -> label mapping,
//! multi-select loop and cancellation, and the handlers' notify-and-`Ok(None)` error path.
//!
//! `ScriptedHost` is the test's `HostServices`: `select` records the prompt and the option rows
//! it was shown and answers from a script; `notify` records; `open_overlay` either takes the
//! overlay (rendering one frame so the test can see what the host would paint) or refuses it;
//! `human_interaction_lock` hands out a real lock or `None`. Everything else is the trait's
//! default (deny).
//!
//! Red before / green after: the overlay/tool pins are green on both sides (they had no test).
//! One test is RED against the pre-change `extension.rs`:
//! `cheatsheet_with_a_bad_pipeline_notifies_the_python_error_and_returns_none` — the message
//! used `{bad:?}` (Rust `"e"`) where the Python's `{selected_pipeline!r}` prints `'e'`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use cyrup_core::{CancelToken, Content, Tool, ToolCallId};
use cyrup_ext::host::{
    DialogOptions, HostServices, HumanInteractionLock, InteractiveOverlay, NotifyKind,
    OverlayColor, OverlayKey, OverlayKeyCode, OverlayOutcome,
};
use cyrup_ext::{ExtMode, HostCtx, NativeExtension};
use cyrup_flux::ask_tool::AskUserQuestionTool;
use cyrup_flux::extension::{FluxExtension, STATUS_OVERLAY_SHORTCUT};
use cyrup_flux::overlay::{FluxStatusOverlay, open_status_overlay};
use cyrup_flux::resources::BundledRoot;
use cyrup_flux::{render_about, render_cheatsheet, render_status, state};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------------------------
// ScriptedHost
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct ScriptedHost {
    answers: Mutex<VecDeque<Option<String>>>,
    select_calls: Mutex<Vec<(String, Vec<String>)>>,
    notify_calls: Mutex<Vec<(String, NotifyKind)>>,
    lock: Option<Arc<HumanInteractionLock>>,
    accept_overlays: bool,
    /// The plain text of the first frame of every overlay this host accepted.
    overlay_frames: Mutex<Vec<Vec<String>>>,
}

impl ScriptedHost {
    fn with_answers(answers: &[Option<&str>]) -> Self {
        Self {
            answers: Mutex::new(answers.iter().map(|a| a.map(str::to_string)).collect()),
            lock: Some(Arc::new(HumanInteractionLock::new())),
            ..Self::default()
        }
    }

    fn select_calls(&self) -> Vec<(String, Vec<String>)> {
        self.select_calls.lock().unwrap().clone()
    }

    fn notify_calls(&self) -> Vec<(String, NotifyKind)> {
        self.notify_calls.lock().unwrap().clone()
    }

    fn overlay_frames(&self) -> Vec<Vec<String>> {
        self.overlay_frames.lock().unwrap().clone()
    }
}

impl HostServices for ScriptedHost {
    fn select(&self, prompt: &str, options: &Value, _opts: &DialogOptions) -> Option<String> {
        let rows: Vec<String> = options
            .as_array()
            .expect("select is handed a flat array of option strings")
            .iter()
            .map(|v| v.as_str().expect("every option is a string").to_string())
            .collect();
        self.select_calls
            .lock()
            .unwrap()
            .push((prompt.to_string(), rows));
        self.answers.lock().unwrap().pop_front().flatten()
    }

    fn notify(&self, message: &str, kind: NotifyKind) {
        self.notify_calls
            .lock()
            .unwrap()
            .push((message.to_string(), kind));
    }

    fn open_overlay(&self, mut overlay: Box<dyn InteractiveOverlay>) -> bool {
        if !self.accept_overlays {
            return false;
        }
        let frame = overlay
            .render(80, 24)
            .iter()
            .map(|l| l.plain_text())
            .collect();
        self.overlay_frames.lock().unwrap().push(frame);
        true
    }

    fn human_interaction_lock(&self) -> Option<Arc<HumanInteractionLock>> {
        self.lock.clone()
    }
}

fn slot(host: ScriptedHost) -> (Arc<OnceLock<Arc<dyn HostServices>>>, Arc<ScriptedHost>) {
    let host = Arc::new(host);
    let services: Arc<dyn HostServices> = host.clone();
    let slot = Arc::new(OnceLock::new());
    slot.set(services).ok().unwrap();
    (slot, host)
}

fn write(base: &Path, rel: &str, content: &str) {
    let path = base.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn small_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let b = dir.path();
    write(
        b,
        "todo/01-alpha.md",
        "---\nstage: exec\nstatus: in-progress\n---\n",
    );
    write(
        b,
        "todo/02-bravo.md",
        "---\nstage: qa\nstatus: needs-rework\n---\n",
    );
    write(b, "todo/03-charlie.md", "---\nstage: aug\n---\n");
    write(
        b,
        "done/2026-04-29-16-57/a-first.md",
        "---\nstage: tests\n---\n",
    );
    write(
        b,
        "done/2026-05-01-09-00/only.md",
        "---\nstage: exec\nstatus: done\n---\n",
    );
    write(b, "review/critical/c1.md", "");
    write(b, "review/medium/m1.md", "");
    dir
}

// ---------------------------------------------------------------------------------------------
// overlay.rs
// ---------------------------------------------------------------------------------------------

/// The overlay's frame IS the plain-text panel, span styling aside: same header title, the same
/// lines in the same order, an `(ESC to close)` hint on the title row, and one extra blank line
/// before the closing rule. Anything the two channels disagree on is a layout-arithmetic drift
/// between `overlay.rs` and `render_status.rs`, which duplicate `STAGE_W`/`SECTION_PAD`/
/// `MIN_PANEL_W`/`sev_col_width` by hand.
#[test]
fn the_overlay_draws_the_same_text_as_the_plain_status_panel() {
    let dir = small_tree();
    let plain: Vec<String> = render_status::render(dir.path(), true, true, true)
        .lines()
        .map(str::to_string)
        .collect();
    let mut overlay = FluxStatusOverlay::with_base(dir.path().to_path_buf());
    let frame: Vec<String> = overlay
        .render(80, 24)
        .iter()
        .map(|l| l.plain_text())
        .collect();

    assert_eq!(frame[0], "\u{1D571} FLUX STATUS   (ESC to close)");
    assert_eq!(plain[0], "\u{1D571} FLUX STATUS");
    assert_eq!(
        frame.len(),
        plain.len() + 1,
        "one extra blank before the rule"
    );
    assert_eq!(
        &frame[1..frame.len() - 2],
        &plain[1..plain.len() - 1],
        "body lines differ between the overlay and /flux/status"
    );
    assert_eq!(frame[frame.len() - 2], "");
    assert_eq!(frame[frame.len() - 1], plain[plain.len() - 1]);
    assert!(
        frame.iter().any(|l| l.contains("🔄  in-progress")),
        "status glyphs must reach the overlay"
    );
}

/// The colour layer `flux_status.py` carries in ANSI (`STATUS_STYLE` `:49-54`, `SEVERITY_COLOR`
/// `:57-62`) maps onto span colours: in-progress is the ORANGE->Yellow collapse, needs-rework
/// red, done green, names cyan, the critical dot red and the medium dot the TEAL->Cyan collapse.
#[test]
fn the_overlay_colours_statuses_names_and_severity_dots() {
    let dir = small_tree();
    let mut overlay = FluxStatusOverlay::with_base(dir.path().to_path_buf());
    let lines = overlay.render(80, 24);
    let span_with = |needle: &str| {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.text.contains(needle))
            .unwrap_or_else(|| panic!("no span containing {needle:?}"))
            .clone()
    };
    assert_eq!(span_with("in-progress").fg, Some(OverlayColor::Yellow));
    assert_eq!(span_with("needs-rework").fg, Some(OverlayColor::Red));
    assert_eq!(span_with("✅  done").fg, Some(OverlayColor::Green));
    let unknown = span_with("(unknown)");
    assert!(unknown.dim && unknown.fg.is_none());
    assert_eq!(span_with("01-alpha").fg, Some(OverlayColor::Cyan));
    let dots: Vec<Option<OverlayColor>> = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter(|s| s.text.starts_with('\u{25CF}'))
        .map(|s| s.fg)
        .collect();
    assert_eq!(
        dots,
        vec![Some(OverlayColor::Red), Some(OverlayColor::Cyan)],
        "critical then medium, in the fixed severity order"
    );
}

/// `tick` re-collects and reports a change ONLY when the model changed (a `true` every 2s
/// would repaint forever); ESC closes, any other key is ignored; the refresh cadence is 2s.
#[test]
fn the_overlay_ticks_only_on_change_and_closes_on_escape() {
    let dir = small_tree();
    let mut overlay = FluxStatusOverlay::with_base(dir.path().to_path_buf());
    assert_eq!(overlay.refresh_ms(), 2000);
    assert!(!overlay.tick(), "nothing changed yet");
    write(
        dir.path(),
        "todo/04-delta.md",
        "---\nstage: exec\nstatus: done\n---\n",
    );
    assert!(overlay.tick(), "a new todo is a change");
    assert!(!overlay.tick(), "and it is a change once");
    let frame: Vec<String> = overlay
        .render(80, 24)
        .iter()
        .map(|l| l.plain_text())
        .collect();
    assert!(frame.iter().any(|l| l.starts_with("04-delta")));
    write(
        dir.path(),
        "todo/04-delta.md",
        "---\nstage: qa\nstatus: done\n---\n",
    );
    assert!(overlay.tick(), "a frontmatter edit is a change");

    assert_eq!(
        overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Escape)),
        OverlayOutcome::Close
    );
    assert_eq!(
        overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Char('q'))),
        OverlayOutcome::Ignored
    );
    assert_eq!(
        overlay.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('c'))),
        OverlayOutcome::Ignored
    );
}

/// `open_status_overlay`'s three outcomes: no host bound -> nothing (no panic, no notify);
/// host takes the overlay -> it is handed over and NOTHING is notified; host refuses (`false`
/// is "no interactive surface", not an error) -> the plain panel for the process's own base
/// arrives as ONE Info notification.
#[test]
fn open_status_overlay_hands_over_or_falls_back_to_the_plain_panel() {
    let unbound: Arc<OnceLock<Arc<dyn HostServices>>> = Arc::new(OnceLock::new());
    open_status_overlay(&unbound);

    let (slot_accepting, accepting) = slot(ScriptedHost {
        accept_overlays: true,
        ..ScriptedHost::default()
    });
    open_status_overlay(&slot_accepting);
    let frames = accepting.overlay_frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0][0], "\u{1D571} FLUX STATUS   (ESC to close)");
    assert!(accepting.notify_calls().is_empty());

    let (slot_refusing, refusing) = slot(ScriptedHost::default());
    open_status_overlay(&slot_refusing);
    assert!(refusing.overlay_frames().is_empty());
    let expected = render_status::render(&state::derive_base(), true, true, true);
    assert_eq!(
        refusing.notify_calls(),
        vec![(expected, NotifyKind::Info)],
        "the fallback is the full plain panel, as Info"
    );
}

// ---------------------------------------------------------------------------------------------
// ask_tool.rs
// ---------------------------------------------------------------------------------------------

fn ask(tool: &AskUserQuestionTool, params: Value, cancel: CancelToken) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        tool.execute(ToolCallId::from("ask-1"), params, cancel, Box::new(|_| {}))
            .await
            .map(|r| {
                assert_eq!(r.content.len(), 1, "one text block");
                match &r.content[0] {
                    Content::Text { text, .. } => text.to_string(),
                    other => panic!("not a text block: {other:?}"),
                }
            })
            .map_err(|e| e.message)
    })
}

fn two_options() -> Value {
    json!([
        {"label": "Alpha", "description": "first choice"},
        {"label": "Beta"}
    ])
}

#[test]
fn ask_tool_metadata_and_schema() {
    let (slot, _) = slot(ScriptedHost::default());
    let tool = AskUserQuestionTool::new(slot);
    assert_eq!(tool.name(), "ask_user_question");
    assert_eq!(tool.label(), Some("Ask"));
    assert!(
        tool.prompt_snippet()
            .unwrap()
            .starts_with("ask_user_question(")
    );
    let schema = tool.parameters();
    assert_eq!(schema["required"], json!(["question", "options"]));
    assert_eq!(schema["properties"]["options"]["minItems"], json!(2));
    assert_eq!(schema["properties"]["options"]["maxItems"], json!(4));
    assert_eq!(
        schema["properties"]["options"]["items"]["required"],
        json!(["label"])
    );
}

/// Every refusal happens BEFORE any dialog: no host bound, malformed parameters, an option
/// count outside 2-4, and a host without the interaction lock.
#[test]
fn ask_tool_refuses_without_a_host_with_bad_params_or_without_the_lock() {
    let unbound: Arc<OnceLock<Arc<dyn HostServices>>> = Arc::new(OnceLock::new());
    let err = ask(
        &AskUserQuestionTool::new(unbound),
        json!({"question": "q", "options": two_options()}),
        CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(err, "ask_user_question: no interactive host");

    let (slot_ok, host) = slot(ScriptedHost::with_answers(&[Some("Alpha — first choice")]));
    let tool = AskUserQuestionTool::new(slot_ok);
    let err = ask(&tool, json!({"question": "q"}), CancelToken::new()).unwrap_err();
    assert!(
        err.starts_with("ask_user_question: invalid parameters: "),
        "{err}"
    );
    let err = ask(
        &tool,
        json!({"question": "q", "options": [{"label": "only"}]}),
        CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(
        err,
        "ask_user_question: `options` must have 2-4 entries, got 1"
    );
    let five: Vec<Value> = (1..=5).map(|i| json!({"label": format!("o{i}")})).collect();
    let err = ask(
        &tool,
        json!({"question": "q", "options": five}),
        CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(
        err,
        "ask_user_question: `options` must have 2-4 entries, got 5"
    );
    assert!(host.select_calls().is_empty(), "no dialog was opened");

    let (slot_no_lock, host) = slot(ScriptedHost {
        answers: Mutex::new(VecDeque::from([Some("Alpha".to_string())])),
        ..ScriptedHost::default()
    });
    let err = ask(
        &AskUserQuestionTool::new(slot_no_lock),
        json!({"question": "q", "options": two_options()}),
        CancelToken::new(),
    )
    .unwrap_err();
    assert_eq!(err, "ask_user_question: interaction lock unavailable");
    assert!(host.select_calls().is_empty());
}

/// Single select: the prompt is `header: question` (or the bare question when the header is
/// absent or blank); the rows shown are `label — description` (or the bare label when the
/// description is absent or blank); the chosen ROW maps back to its LABEL; a host that echoes
/// the bare label instead of the row still resolves; a cancelled dialog is the cancelled text.
#[test]
fn ask_tool_single_select_maps_the_display_row_back_to_its_label() {
    let (slot_a, host) = slot(ScriptedHost::with_answers(&[
        Some("Alpha — first choice"),
        Some("Beta"),
        None,
    ]));
    let tool = AskUserQuestionTool::new(slot_a);
    let params = json!({
        "question": "Which one?",
        "header": "Pick",
        "options": two_options()
    });
    assert_eq!(
        ask(&tool, params.clone(), CancelToken::new()).unwrap(),
        "Alpha"
    );
    assert_eq!(
        ask(&tool, params.clone(), CancelToken::new()).unwrap(),
        "Beta"
    );
    assert_eq!(
        ask(&tool, params, CancelToken::new()).unwrap(),
        "(cancelled — no selection made)"
    );
    let calls = host.select_calls();
    assert_eq!(calls.len(), 3);
    for (prompt, rows) in &calls {
        assert_eq!(prompt, "Pick: Which one?");
        assert_eq!(rows, &["Alpha — first choice", "Beta"]);
    }

    let (slot_b, host) = slot(ScriptedHost::with_answers(&[Some("Beta")]));
    let tool = AskUserQuestionTool::new(slot_b);
    let out = ask(
        &tool,
        json!({
            "question": "Which one?",
            "header": "   ",
            "options": [{"label": "Alpha", "description": "  "}, {"label": "Beta"}]
        }),
        CancelToken::new(),
    )
    .unwrap();
    assert_eq!(out, "Beta");
    assert_eq!(
        host.select_calls(),
        vec![(
            "Which one?".to_string(),
            vec!["Alpha".to_string(), "Beta".to_string()]
        )]
    );
}

/// Multi select: every round offers `✔ Done` first and then the options not yet chosen; picking
/// Done returns the picks in pick order, joined by `, `; picking every option ends the loop
/// without another round; a cancelled round cancels the whole answer.
#[test]
fn ask_tool_multi_select_loops_until_done_exhausted_or_cancelled() {
    let options = json!([{"label": "A"}, {"label": "B"}, {"label": "C"}]);
    let params = json!({"question": "Many?", "options": options, "multiple": true});

    let (slot_done, host) = slot(ScriptedHost::with_answers(&[
        Some("B"),
        Some("A"),
        Some("\u{2714} Done"),
    ]));
    assert_eq!(
        ask(
            &AskUserQuestionTool::new(slot_done),
            params.clone(),
            CancelToken::new()
        )
        .unwrap(),
        "B, A"
    );
    let rows: Vec<Vec<String>> = host.select_calls().into_iter().map(|(_, r)| r).collect();
    assert_eq!(
        rows,
        vec![
            vec!["\u{2714} Done", "A", "B", "C"],
            vec!["\u{2714} Done", "A", "C"],
            vec!["\u{2714} Done", "C"],
        ]
        .into_iter()
        .map(|r| r.into_iter().map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>()
    );

    let (slot_all, host) = slot(ScriptedHost::with_answers(&[
        Some("C"),
        Some("A"),
        Some("B"),
    ]));
    assert_eq!(
        ask(
            &AskUserQuestionTool::new(slot_all),
            params.clone(),
            CancelToken::new()
        )
        .unwrap(),
        "C, A, B"
    );
    assert_eq!(
        host.select_calls().len(),
        3,
        "no fourth round once exhausted"
    );

    let (slot_done_first, host) = slot(ScriptedHost::with_answers(&[Some("\u{2714} Done")]));
    assert_eq!(
        ask(
            &AskUserQuestionTool::new(slot_done_first),
            params.clone(),
            CancelToken::new()
        )
        .unwrap(),
        "",
        "Done with nothing picked is the empty answer"
    );
    assert_eq!(host.select_calls().len(), 1);

    let (slot_cancel, host) = slot(ScriptedHost::with_answers(&[Some("A"), None]));
    assert_eq!(
        ask(
            &AskUserQuestionTool::new(slot_cancel),
            params,
            CancelToken::new()
        )
        .unwrap(),
        "(cancelled — no selection made)"
    );
    assert_eq!(host.select_calls().len(), 2);
}

/// A token that is already cancelled when the tool runs never opens a dialog, in either mode.
#[test]
fn ask_tool_honours_a_pre_cancelled_token_without_opening_a_dialog() {
    for multiple in [false, true] {
        let (slot_c, host) = slot(ScriptedHost::with_answers(&[Some("Alpha — first choice")]));
        let cancel = CancelToken::new();
        cancel.cancel();
        let out = ask(
            &AskUserQuestionTool::new(slot_c),
            json!({"question": "q", "options": two_options(), "multiple": multiple}),
            cancel,
        )
        .unwrap();
        assert_eq!(
            out, "(cancelled — no selection made)",
            "multiple={multiple}"
        );
        assert!(host.select_calls().is_empty(), "multiple={multiple}");
    }
}

// ---------------------------------------------------------------------------------------------
// extension.rs — command routing
// ---------------------------------------------------------------------------------------------

fn extension_with(
    host: ScriptedHost,
) -> (Arc<FluxExtension>, Arc<ScriptedHost>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let ext = cyrup_flux::flux_extension_with_root(BundledRoot::Vendored(dir.path().to_path_buf()));
    let host = Arc::new(host);
    let services: Arc<dyn HostServices> = host.clone();
    ext.set_host_services(services);
    (ext, host, dir)
}

fn command_ctx() -> HostCtx {
    HostCtx::command(ExtMode::Tui, true, std::env::temp_dir())
}

fn run_command(
    ext: &FluxExtension,
    name: &str,
    args: &str,
    ctx: &HostCtx,
) -> Result<Option<String>, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(ext.execute_command(name, args, ctx))
        .map_err(|e| e.to_string())
}

/// `/flux/about` and `/flux/cheatsheet` (valid filters) return the renderers' text verbatim
/// as the command reply; `/flux/cheatsheet`'s filter is trimmed and case-folded on the way.
#[test]
fn about_and_cheatsheet_commands_reply_with_the_rendered_panels() {
    let (ext, host, _dir) = extension_with(ScriptedHost::default());
    let ctx = command_ctx();
    assert_eq!(
        run_command(&ext, "flux/about", "", &ctx).unwrap(),
        Some(render_about::render())
    );
    assert_eq!(
        run_command(&ext, "flux/cheatsheet", "", &ctx).unwrap(),
        Some(render_cheatsheet::render(None))
    );
    assert_eq!(
        run_command(&ext, "flux/cheatsheet", " b ", &ctx).unwrap(),
        Some(render_cheatsheet::render(Some("B")))
    );
    assert!(host.notify_calls().is_empty());
}

/// `flux_cheatsheet.py:207-209`: `invalid pipeline: 'e' (choose from A, B, C, D)` — the raw
/// argument in Python `repr` quotes — as a self-issued Error notification, with `Ok(None)` so
/// the host adds no `command:…:` prefix of its own.
#[test]
fn cheatsheet_with_a_bad_pipeline_notifies_the_python_error_and_returns_none() {
    let (ext, host, _dir) = extension_with(ScriptedHost::default());
    assert_eq!(
        run_command(&ext, "flux/cheatsheet", "e", &command_ctx()).unwrap(),
        None
    );
    assert_eq!(
        host.notify_calls(),
        vec![(
            "invalid pipeline: 'e' (choose from A, B, C, D)".to_string(),
            NotifyKind::Error
        )]
    );
}

/// `flux_status.py:323-326`: `invalid section(s): bogus, zzz (choose from done, review, todo)`
/// — bad names sorted and deduped, valid names listed sorted — as an Error notification with
/// `Ok(None)`.
#[test]
fn status_with_bad_sections_notifies_the_python_error_and_returns_none() {
    let (ext, host, _dir) = extension_with(ScriptedHost::default());
    assert_eq!(
        run_command(
            &ext,
            "flux/status",
            "todo bogus done zzz bogus",
            &command_ctx()
        )
        .unwrap(),
        None
    );
    assert_eq!(
        host.notify_calls(),
        vec![(
            "invalid section(s): bogus, zzz (choose from done, review, todo)".to_string(),
            NotifyKind::Error
        )]
    );
}

/// An unregistered name is an error, and every handler runs at command tier only — an
/// event-tier `HostCtx` is refused before any rendering happens.
#[test]
fn unknown_commands_and_event_tier_contexts_are_refused() {
    let (ext, host, _dir) = extension_with(ScriptedHost::default());
    let err = run_command(&ext, "flux/nope", "", &command_ctx()).unwrap_err();
    assert!(err.contains("no handler for command `flux/nope`"), "{err}");
    let event_ctx = HostCtx::event(ExtMode::Tui, true, std::env::temp_dir());
    assert!(run_command(&ext, "flux/about", "", &event_ctx).is_err());
    assert!(host.notify_calls().is_empty());
}

/// The registered chord opens the overlay through the bound host (the accepted path — the
/// refused path and the retired chord are `flux_004_status_shortcut.rs`'s).
#[test]
fn the_status_shortcut_opens_the_overlay_on_an_accepting_host() {
    let (ext, host, _dir) = extension_with(ScriptedHost {
        accept_overlays: true,
        ..ScriptedHost::default()
    });
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(ext.execute_shortcut(STATUS_OVERLAY_SHORTCUT, &command_ctx()))
        .unwrap();
    let frames = host.overlay_frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0][0], "\u{1D571} FLUX STATUS   (ESC to close)");
    assert!(host.notify_calls().is_empty());
}
