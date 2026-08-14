//! EXT-006 END TO END — a renderer an extension registers must actually DRAW.
//!
//! Pi resolves the renderer at the point of display, in two places:
//!
//! * a custom message → `const renderer = this.session.extensionRunner.getMessageRenderer(
//!   message.customType); new CustomMessageComponent(message, renderer, …)`
//!   (`modes/interactive/interactive-mode.ts:3324-3336`) — the resolved renderer REPLACES the
//!   default `[label] body` framing;
//! * a tool row → the extension's `ToolDefinition.renderCall`/`renderResult`
//!   (`extensions/types.ts:472-481`), preferred over the built-in by
//!   `modes/interactive/components/tool-execution.ts:81-112`.
//!
//! cyrup recorded both registrations and consulted neither: `ToolDescriptor.has_renderer` was a
//! field nothing read, `register_message_renderer` wrote to a per-guest `Vec` with no consumer, and
//! `LiveExtension::render_call` had exactly one caller — a unit test holding the handle directly.
//! `Entry::Custom` always drew the default framing and the tool row always drew the built-in shell.
//!
//! These tests assert on the RENDERED TERMINAL CELLS: the extension's text is on screen and the
//! default framing is not. Nothing here asserts that a registration returned `Ok`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_agent::AgentMessage;
use cyrup_core::{ExtensionId, ToolCallId};
use cyrup_ext::{
    ExtError, ExtMode, ExtensionHost, HostConfig, HostCtx, HostEvent, HookOutcome, InitApi,
    NativeExtension,
};
use cyrup_session_svc::AgentSessionEvent;
use crate::{App, UiTheme};
use ratatui::backend::TestBackend;
use serde_json::{json, Value};

/// An extension that renders BOTH surfaces: custom messages of type `demo`, and the `bash` tool —
/// a BUILT-IN, so the test also proves an extension can take over a tool cyrup already draws.
struct RendererExt;

#[async_trait::async_trait]
impl NativeExtension for RendererExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("renderer-demo")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_message_renderer("demo");
        api.register_tool_renderer("bash");
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    // Both renderers DERIVE from the payload (a byte count) rather than echoing it, so a
    // "the built-in did not also draw" assertion cannot be satisfied by this text quoting the
    // built-in's own marker back.
    fn render_call(&self, key: &str, call: &Value) -> Option<Value> {
        Some(Value::String(format!("EXTCALL[{key}] payload-bytes={}", weigh(call))))
    }

    fn render_result(&self, key: &str, result: &Value) -> Option<Value> {
        Some(Value::String(format!("EXTRESULT[{key}] payload-bytes={}", weigh(result))))
    }
}

/// The serialized size of the payload the host handed the renderer — proof the real event data
/// crossed into the renderer, without reproducing any of its text on screen.
fn weigh(v: &Value) -> usize {
    serde_json::to_string(v).unwrap_or_default().len()
}

async fn host_with_renderer() -> Arc<ExtensionHost> {
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }));
    host.load_native(Arc::new(RendererExt)).await.unwrap();
    host
}

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(90, 20), UiTheme::dark()).unwrap()
}

/// Flatten the test backend's cell grid (the live region) into text.
fn buffer_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

// =============================================================================================

/// A custom message whose type an extension registered a renderer for draws the EXTENSION's text —
/// not the default `[demo] …` labeled block.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_registered_message_renderer_draws_the_custom_message() {
    let host = host_with_renderer().await;
    let mut app = app();

    let ev = AgentSessionEvent::MessageEnd {
        message: AgentMessage::Custom {
            kind: "demo".into(),
            payload: json!("plain fallback body"),
            timestamp: None,
        },
    };
    app.ingest_event_with_extensions(&ev, &host).await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(
        sb.contains("EXTCALL[demo] payload-bytes="),
        "the extension's renderer produced the transcript block:\n{sb}"
    );
    assert!(
        !sb.contains("plain fallback body"),
        "the default `[label] body` framing did NOT also draw:\n{sb}"
    );
}

/// An UNREGISTERED custom type still draws the default framing — the fix must not hijack messages
/// no extension claimed (Pi: `getMessageRenderer` returns undefined ⇒ the default component).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unclaimed_custom_type_keeps_the_default_framing() {
    let host = host_with_renderer().await;
    let mut app = app();

    let ev = AgentSessionEvent::MessageEnd {
        message: AgentMessage::Custom {
            kind: "nobody-renders-this".into(),
            payload: json!("plain fallback body"),
            timestamp: None,
        },
    };
    app.ingest_event_with_extensions(&ev, &host).await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(sb.contains("plain fallback body"), "the default body drew:\n{sb}");
    assert!(!sb.contains("EXTCALL"), "no renderer was consulted for an unclaimed type:\n{sb}");
}

/// A tool an extension registered a renderer for draws the EXTENSION's call header and result body
/// instead of cyrup's built-in `bash` block (`$ command` + output tail + `Took …`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_registered_tool_renderer_draws_the_tool_row() {
    let host = host_with_renderer().await;
    let mut app = app();

    let start = AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from("call-1"),
        tool_name: "bash".into(),
        args: json!({ "command": "echo secret-builtin-marker" }),
    };
    app.ingest_event_with_extensions(&start, &host).await;
    app.draw().unwrap();
    let live = buffer_text(&app);
    assert!(
        live.contains("EXTCALL[bash]"),
        "the extension rendered the LIVE tool row (the built-in `$ …` header is replaced):\n{live}"
    );
    assert!(
        !live.contains("$ echo secret-builtin-marker"),
        "the built-in bash header did not also draw:\n{live}"
    );

    let end = AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: ToolCallId::from("call-1"),
        tool_name: "bash".into(),
        is_error: false,
        result: json!({ "content": [{ "type": "text", "text": "builtin-output-marker" }] }),
    };
    app.ingest_event_with_extensions(&end, &host).await;
    app.draw().unwrap();

    // The finished tool flushes to scrollback; look in both places so the assertion does not depend
    // on which side of the commit boundary the frame landed on.
    let seen = format!("{}\n{}", buffer_text(&app), app.scrollback_text());
    assert!(
        seen.contains("EXTRESULT[bash]"),
        "the extension rendered the tool RESULT body:\n{seen}"
    );
    assert!(
        !seen.contains("builtin-output-marker"),
        "the built-in result body did not also draw:\n{seen}"
    );
}

/// A tool NO extension claimed keeps its built-in rendering.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unclaimed_tool_keeps_its_builtin_rendering() {
    let host = host_with_renderer().await;
    let mut app = app();

    let start = AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from("call-2"),
        tool_name: "read".into(),
        args: json!({ "path": "src/main.rs", "offset": 10, "limit": 5 }),
    };
    app.ingest_event_with_extensions(&start, &host).await;
    app.draw().unwrap();

    let live = buffer_text(&app);
    assert!(live.contains("read src/main.rs:10-14"), "the built-in read header drew:\n{live}");
    assert!(!live.contains("EXTCALL"), "no renderer was consulted for an unclaimed tool:\n{live}");
}

// =============================================================================================
// The SERIALIZED WIDGET TREE path (EXT-006 blocker).
//
// A guest renderer cannot hand back a live `pi-tui` `Component` the way a Pi renderer does
// (`custom-message.ts:66-81` adds the returned `Component` as a child; nothing is stringified), so
// cyrup's wire analog is that tree SERIALIZED — the shape `cyrup_ext_sdk::widget` builds and the
// shape both WIT world copies document above `render-call`. The host is what has to turn it back
// into rows. Before this fix the TUI only special-cased `Value::String` and pretty-printed
// everything else, so a guest following cyrup's OWN SDK example drew a raw JSON blob.
//
// These tests exercise the tree shapes through the SAME production fold as the tests above.
// =============================================================================================

/// Renders both surfaces with real widget TREES rather than bare strings.
struct WidgetRendererExt;

#[async_trait::async_trait]
impl NativeExtension for WidgetRendererExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("widget-renderer")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_message_renderer("demo");
        api.register_tool_renderer("bash");
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    /// A `Container` of a header row, a blank row, and a nested `HStack` — every constructor the
    /// vocabulary has, so one assertion covers stacking, spacing and side-by-side joining.
    fn render_call(&self, _key: &str, _call: &Value) -> Option<Value> {
        Some(json!({
            "widget": "container",
            "children": [
                { "widget": "text", "text": "WIDGET-HEADER" },
                { "widget": "spacer", "lines": 2 },
                { "widget": "hstack", "children": [
                    { "widget": "text", "text": "left-" },
                    { "widget": "truncated-text", "text": "right" },
                ]},
            ]
        }))
    }

    /// A bare array is the documented shorthand for a stack.
    fn render_result(&self, _key: &str, _result: &Value) -> Option<Value> {
        Some(json!([
            { "widget": "markdown", "text": "WIDGET-RESULT-BODY" },
        ]))
    }
}

/// A renderer that returns a node the vocabulary does not cover. The host must keep drawing
/// SOMETHING (the JSON) rather than a blank row, so an author sees their typo.
struct UnknownWidgetExt;

#[async_trait::async_trait]
impl NativeExtension for UnknownWidgetExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("unknown-widget")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_message_renderer("demo");
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn render_call(&self, _key: &str, _call: &Value) -> Option<Value> {
        Some(json!({ "widget": "hologram", "text": "MISTYPED-NODE" }))
    }
}

async fn host_with(ext: Arc<dyn NativeExtension>) -> Arc<ExtensionHost> {
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }));
    host.load_native(ext).await.unwrap();
    host
}

/// The widget TREE a renderer returns draws as ROWS — the header, the blank rows the `Spacer`
/// asked for, and the `HStack`'s children joined on ONE row. No JSON punctuation reaches the
/// screen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_widget_tree_draws_as_rows_not_as_json() {
    let host = host_with(Arc::new(WidgetRendererExt)).await;
    let mut app = app();

    let ev = AgentSessionEvent::MessageEnd {
        message: AgentMessage::Custom {
            kind: "demo".into(),
            payload: json!("plain fallback body"),
            timestamp: None,
        },
    };
    app.ingest_event_with_extensions(&ev, &host).await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(sb.contains("WIDGET-HEADER"), "the `text` node drew:\n{sb}");
    // The `hstack` joined its two children on one row — `left-` and `right` are adjacent, not
    // stacked, which is the whole point of the tag.
    assert!(sb.contains("left-right"), "the `hstack` joined its children on ONE row:\n{sb}");
    // The `spacer(2)` put TWO blank rows between the header and the hstack row.
    assert!(
        sb.contains("WIDGET-HEADER\n\n\nleft-right"),
        "the `spacer` produced its blank rows in place:\n{sb:?}"
    );
    // The regression this test exists for: the tree must not be pretty-printed into the transcript.
    assert!(
        !sb.contains("\"widget\""),
        "the serialized tree was FLATTENED, not dumped as JSON:\n{sb}"
    );
    assert!(!sb.contains("plain fallback body"), "the default framing did not also draw:\n{sb}");
}

/// The same vocabulary on the TOOL surface, through `push_tool_*` rather than the custom-message
/// path — the two feed different transcript entries, so both need the flattened text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_widget_tree_draws_on_the_tool_surface_too() {
    let host = host_with(Arc::new(WidgetRendererExt)).await;
    let mut app = app();

    let start = AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from("call-w"),
        tool_name: "bash".into(),
        args: json!({ "command": "echo secret-builtin-marker" }),
    };
    app.ingest_event_with_extensions(&start, &host).await;
    app.draw().unwrap();
    let live = buffer_text(&app);
    assert!(live.contains("WIDGET-HEADER"), "the tool row drew the widget tree:\n{live}");
    assert!(!live.contains("\"widget\""), "not as JSON:\n{live}");

    let end = AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: ToolCallId::from("call-w"),
        tool_name: "bash".into(),
        is_error: false,
        result: json!({ "content": [{ "type": "text", "text": "builtin-output-marker" }] }),
    };
    app.ingest_event_with_extensions(&end, &host).await;
    app.draw().unwrap();

    let seen = format!("{}\n{}", buffer_text(&app), app.scrollback_text());
    assert!(seen.contains("WIDGET-RESULT-BODY"), "the bare-array shorthand drew:\n{seen}");
    assert!(!seen.contains("\"markdown\""), "not as JSON:\n{seen}");
}

/// An unrecognized node is DRAWN as JSON, not dropped — the deliberate fallback. Pinned so a future
/// change cannot quietly turn a mistyped node into a blank row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_widget_tag_is_still_visible() {
    let host = host_with(Arc::new(UnknownWidgetExt)).await;
    let mut app = app();

    let ev = AgentSessionEvent::MessageEnd {
        message: AgentMessage::Custom {
            kind: "demo".into(),
            payload: json!("plain fallback body"),
            timestamp: None,
        },
    };
    app.ingest_event_with_extensions(&ev, &host).await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(sb.contains("MISTYPED-NODE"), "the unrecognized node is visible, not dropped:\n{sb}");
}

// ============================================================ X11 — the REPLAY arm ============

/// **X11 — a RESUMED session keeps its extension rendering.**
///
/// Pi's replay walk performs the SAME lookup as the live path, inside the same `display` gate:
///
/// ```ts
/// case "custom": {
///     if (message.display) {
///         const renderer = this.session.extensionRunner.getMessageRenderer(message.customType);
///         const component = new CustomMessageComponent(message, renderer, …);
/// ```
/// (`modes/interactive/interactive-mode.ts:3469-3477`).
///
/// cyrup's replay arm called `push_custom_message`, which hard-codes `Rendered::None`, so every
/// `/resume`, `/fork`, `/import` and `--continue` silently downgraded extension-rendered blocks to
/// the built-in `[type] body` framing — a session looked different after a resume than before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_custom_message_still_reaches_its_registered_renderer() {
    use cyrup_session_svc::agent_message::{AgentMessage as SessionMessage, CustomRoleMessage};

    let host = host_with_renderer().await;
    let mut app = app();

    app.replay_session_with_extensions(
        &[SessionMessage::Custom(CustomRoleMessage {
            custom_type: "demo".to_string(),
            content: json!("plain fallback body"),
            display: true,
            details: None,
            timestamp: 0,
        })],
        &host,
    )
    .await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(
        sb.contains("EXTCALL[demo] payload-bytes="),
        "the replayed message went through `getMessageRenderer` (`:3471`):\n{sb}"
    );
    assert!(
        !sb.contains("plain fallback body"),
        "the default `[label] body` framing did NOT also draw:\n{sb}"
    );
}

/// MIRROR 1 — a replayed custom type NO extension claimed still draws the built-in framing
/// (`getMessageRenderer` → `undefined` ⇒ `CustomMessageComponent`'s default box).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_unclaimed_custom_type_keeps_the_default_framing() {
    use cyrup_session_svc::agent_message::{AgentMessage as SessionMessage, CustomRoleMessage};

    let host = host_with_renderer().await;
    let mut app = app();

    app.replay_session_with_extensions(
        &[SessionMessage::Custom(CustomRoleMessage {
            custom_type: "nobody-renders-this".to_string(),
            content: json!("plain fallback body"),
            display: true,
            details: None,
            timestamp: 0,
        })],
        &host,
    )
    .await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(sb.contains("plain fallback body"), "the default body drew:\n{sb}");
    assert!(!sb.contains("EXTCALL"), "no renderer was consulted for an unclaimed type:\n{sb}");
}

/// MIRROR 2 — `display: false` is still the outer gate (`:3470`): the renderer is not consulted and
/// nothing at all is drawn, so the fix did not move the lookup outside its `if`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_undisplayed_custom_message_renders_nothing() {
    use cyrup_session_svc::agent_message::{AgentMessage as SessionMessage, CustomRoleMessage};

    let host = host_with_renderer().await;
    let mut app = app();

    app.replay_session_with_extensions(
        &[SessionMessage::Custom(CustomRoleMessage {
            custom_type: "demo".to_string(),
            content: json!("plain fallback body"),
            display: false,
            details: None,
            timestamp: 0,
        })],
        &host,
    )
    .await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(!sb.contains("EXTCALL"), "no renderer ran for a non-display message:\n{sb}");
    assert!(!sb.contains("plain fallback body"), "and no default box either:\n{sb}");
}

/// MIRROR 3 — the renderer sees the message at the position it occupies in the replay, so a walk
/// carrying several custom messages does not cross their rendered texts. The two payloads differ in
/// size, and the renderer reports the byte count it was handed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_replayed_custom_message_gets_its_own_renderer_output() {
    use cyrup_session_svc::agent_message::{AgentMessage as SessionMessage, CustomRoleMessage};

    let host = host_with_renderer().await;
    let mut app = app();

    let msg = |content: Value| {
        SessionMessage::Custom(CustomRoleMessage {
            custom_type: "demo".to_string(),
            content,
            display: true,
            details: None,
            timestamp: 0,
        })
    };
    app.replay_session_with_extensions(
        &[msg(json!("s")), msg(json!("a much longer payload than the first one"))],
        &host,
    )
    .await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    let sizes: Vec<usize> = sb
        .lines()
        .filter_map(|l| l.split("payload-bytes=").nth(1))
        .filter_map(|n| n.trim().parse::<usize>().ok())
        .collect();
    assert_eq!(sizes.len(), 2, "both messages rendered:\n{sb}");
    assert!(sizes[0] < sizes[1], "each got ITS OWN payload, in order: {sizes:?}\n{sb}");
}

// =============================================================================================
// X15 — the custom-ENTRY surface. Upstream's `addCustomEntryToChat`
// (`interactive-mode.ts:3431-3450`) is the ONLY constructor of `CustomEntryComponent`, and
// `CustomEntryComponent.rebuild` (`custom-entry.ts:40-60`) is the ONLY place in `pi/packages` that
// draws `renderer failed` (`:50`). Its three outcomes draw three different things, and cyrup
// collapsed two of them: `render_via` reported a faulting renderer as `None`, which is also "no
// renderer registered", so `Rendered::Failed` had no producer anywhere in `crates/`.
// =============================================================================================

/// Registers ENTRY renderers only: `card` draws, `boom` panics (upstream's `throw`).
struct EntryRendererExt;

#[async_trait::async_trait]
impl NativeExtension for EntryRendererExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("entry-demo")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_entry_renderer("card");
        api.register_entry_renderer("boom");
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    fn render_entry(&self, custom_type: &str, entry: &Value) -> Option<Value> {
        match custom_type {
            "card" => {
                Some(Value::String(format!("ENTRYCARD payload-bytes={}", weigh(entry))))
            }
            "boom" => panic!("entry renderer exploded"),
            _ => None,
        }
    }
}

async fn host_with_entry_renderer() -> Arc<ExtensionHost> {
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }));
    host.load_native(Arc::new(EntryRendererExt)).await.unwrap();
    host
}

/// The persisted shape `LiveHostServices::append_entry` puts on the wire: the serde tag is
/// `"custom"` and the renderer key lives in `customType` (`cyrup-session`'s `KnownEntry::Custom`,
/// `#[serde(tag = "type", rename_all_fields = "camelCase")]`). Upstream reads exactly this field
/// (`getEntryRenderer(entry.customType)`, `interactive-mode.ts:3432`).
fn entry_event(custom_type: &str) -> AgentSessionEvent {
    AgentSessionEvent::EntryAppended {
        entry: json!({
            "type": "custom",
            "id": "e1",
            "parentId": Value::Null,
            "timestamp": "1970-01-01T00:00:00.000Z",
            "customType": custom_type,
            "data": { "k": "v" },
        }),
    }
}

/// THE REGRESSION, end to end. A panicking ENTRY renderer draws Pi's failure box
/// (`custom-entry.ts:47-52`): `[${customType}] renderer failed: ${message}`. Before X15 the fault
/// arrived as `None` and this drew the plain "entry appended" receipt instead — indistinguishable
/// from an entry nobody renders.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_entry_renderer_draws_the_failure_box() {
    let host = host_with_entry_renderer().await;
    let mut app = app();

    app.ingest_event_with_extensions(&entry_event("boom"), &host).await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(
        sb.contains("[boom] renderer failed: entry renderer exploded"),
        "`custom-entry.ts:50` verbatim — label, the words `renderer failed`, and the throw's own \
         message:\n{sb}"
    );
    assert!(
        !sb.contains("entry appended"),
        "the failure box REPLACES the receipt; a faulting renderer must not read as an \
         unrendered entry:\n{sb}"
    );
}

/// The success arm, so the failure box cannot be satisfied by drawing a box for everything:
/// a renderer that returns a component draws its output (`custom-entry.ts:58-60`) and no failure
/// text at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_working_entry_renderer_draws_its_own_output_and_no_failure_box() {
    let host = host_with_entry_renderer().await;
    let mut app = app();

    app.ingest_event_with_extensions(&entry_event("card"), &host).await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(sb.contains("ENTRYCARD payload-bytes="), "the extension's own output drew:\n{sb}");
    assert!(!sb.contains("renderer failed"), "a working renderer draws no failure box:\n{sb}");
    assert!(!sb.contains("entry appended"), "and no receipt either:\n{sb}");
}

/// The OTHER half of the regression: an entry type NO extension claims must NOT draw the failure
/// box. Upstream draws nothing at all for it (`if (!renderer) { return; }`,
/// `interactive-mode.ts:3433-3435`); cyrup keeps its one-line receipt (a documented delta), and the
/// point of the assertion is that "absent" and "faulted" produce DIFFERENT output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unclaimed_entry_type_never_draws_the_failure_box() {
    let host = host_with_entry_renderer().await;
    let mut app = app();

    app.ingest_event_with_extensions(&entry_event("nobody-renders-this"), &host).await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(
        !sb.contains("renderer failed"),
        "no renderer ran, so nothing threw — this is `RenderOutcome::None`, not `Failed`:\n{sb}"
    );
    assert!(
        sb.contains("entry appended → nobody-renders-this"),
        "the receipt names the entry's `customType`, not the serde tag `custom`:\n{sb}"
    );
}

/// An extension that registered a MESSAGE renderer for a type must not have it invoked for an
/// ENTRY of the same type (upstream keeps `messageRenderers`/`entryRenderers` disjoint,
/// `types.ts:1703-1704`), and the message surface keeps its own upstream behaviour: a custom
/// MESSAGE whose renderer throws falls through to the default box
/// (`custom-message.ts:82-84`) rather than drawing a failure box.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_faulting_message_renderer_still_falls_through_to_the_default_box() {
    struct ThrowingMessageExt;

    #[async_trait::async_trait]
    impl NativeExtension for ThrowingMessageExt {
        fn id(&self) -> ExtensionId {
            ExtensionId::from("throwing-message")
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
            api.register_message_renderer("demo");
            Ok(())
        }
        async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            HookOutcome::Noop
        }
        fn render_call(&self, _key: &str, _call: &Value) -> Option<Value> {
            panic!("message renderer exploded")
        }
    }

    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }));
    host.load_native(Arc::new(ThrowingMessageExt)).await.unwrap();

    let mut app = app();
    let ev = AgentSessionEvent::MessageEnd {
        message: AgentMessage::Custom {
            kind: "demo".into(),
            payload: json!("plain fallback body"),
            timestamp: None,
        },
    };
    app.ingest_event_with_extensions(&ev, &host).await;
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(
        sb.contains("plain fallback body"),
        "`custom-message.ts:82-84` falls through to default rendering — the default box drew:\n{sb}"
    );
    assert!(
        !sb.contains("renderer failed"),
        "the failure box belongs to the ENTRY surface ONLY; drawing it here would be a \
         divergence from `custom-message.ts:82-84`:\n{sb}"
    );
}
