//! The fire-and-forget half of the `ui` capability (blocking half: [`super::rpc_ui_dialogs`]):
//! `notify`/`setStatus`/`setWidget`/`setTitle`/`set_editor_text` reach the wire with no correlated
//! response, `set-header`/`set-footer`/`set-tools-expanded` deliberately never do, and each emitted
//! request carries pi's own field shape — absent keys where pi's `JSON.stringify` drops an
//! `undefined` property, never `null`.

use std::sync::Arc;

use super::support::{build_runtime, fixture, read_json_line, spawn_rpc_duplex};
use cyrup_provider::faux::FauxProvider;

/// The fire-and-forget half of the `ui` capability (`notify`/`set-status`/`set-widget`/`set-title`/
/// `set-editor-text`/`paste-editor-text`) must ALSO reach the RPC client, exactly like Pi's own
/// `notify`/`setStatus`/`setWidget`/`setTitle`/`setEditorText` RPC handlers, each of which just calls
/// `output({type:"extension_ui_request", id, method, ...})` inline with no correlated response
/// expected (`rpc-mode.ts:149-241`) — unlike the blocking `confirm`/`input`/`select`/`editor`
/// half of the capability, whose transport [`super::rpc_ui_dialogs`] exercises, none of these
/// calls block on a reply, so no `extension_ui_response` is ever sent back for them in this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_fire_and_forget_ui_effects_reach_the_wire() {
    use cyrup_ext::host::{HostServices, NotifyKind};
    use tokio::io::AsyncWriteExt;

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let session = runtime.session().await;
    let host_services = session.services().host_services.clone();

    let (mut client_tx, mut client_reader, rpc) = spawn_rpc_duplex(runtime);

    // A get_state first proves the loop (and its effect sink) is up before any effect fires.
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"boot\"}\n").await.unwrap();
    let boot = read_json_line(&mut client_reader).await;
    assert_eq!(boot["command"], "get_state");

    // notify → `{method:"notify", message, notifyType}` (rpc-mode.ts:149-157). None of these calls
    // block: `HostServices::notify` is a plain sync fire-and-forget send, called directly (no
    // `spawn_blocking` needed, unlike the blocking `confirm`/`input`/`select`/`editor` dialogs in
    // `rpc_ui_dialogs`).
    host_services.notify("careful now", NotifyKind::Warning);
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["type"], "extension_ui_request");
    assert_eq!(req["method"], "notify");
    assert_eq!(req["message"], "careful now");
    assert_eq!(req["notifyType"], "warning");

    // set_status(key, Some(text)) → `{method:"setStatus", statusKey, statusText}` (rpc-mode.ts:163-172).
    host_services.set_status("git", Some("main"));
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "setStatus");
    assert_eq!(req["statusKey"], "git");
    assert_eq!(req["statusText"], "main");

    // set_status(key, None) clears the key → `statusText` is OMITTED entirely (not `null`), matching
    // Pi's `JSON.stringify` dropping an `undefined` property.
    host_services.set_status("git", None);
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "setStatus");
    assert_eq!(req["statusKey"], "git");
    assert!(req.get("statusText").is_none(), "a cleared status must omit statusText: {req:?}");

    // SEAM-028/SEAM-011 — the WIT now carries pi's three `setWidget` arguments separately, so this
    // case asserts pi's real member: `widgetKey: string; widgetLines: string[] | undefined;
    // widgetPlacement?: "aboveEditor" | "belowEditor"` (`modes/rpc/rpc-types.ts:264-271` @v0.83.0),
    // with NO `widget` key on any member of the union. Do NOT re-point it at `req["widget"]`.
    host_services.set_widget(
        "todo",
        Some(&["one".to_string(), "two".to_string()]),
        cyrup_ext::host::WidgetPlacement::AboveEditor,
    );
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "setWidget");
    assert_eq!(req["widgetKey"], "todo");
    assert_eq!(req["widgetLines"], serde_json::json!(["one", "two"]));
    assert!(
        req.get("widget").is_none(),
        "pi's setWidget union member carries no `widget` key: {req:?}"
    );
    // `aboveEditor` is pi's documented default for an ABSENT `options.placement`
    // (`extensions/types.ts:107-110`), and pi's `widgetPlacement: options?.placement` then omits the
    // key — see the `[CYRUP-DELTA]` on the emitter.
    assert!(
        req.get("widgetPlacement").is_none(),
        "the default placement is emitted as an absent key, as pi's `options?.placement` is: {req:?}"
    );

    // `content: undefined` is pi's REMOVE (`interactive-mode.ts:1935-1938`), and `JSON.stringify`
    // drops the property — so `widgetLines` is ABSENT, never `null` (SEAM-053's rule).
    host_services.set_widget("todo", None, cyrup_ext::host::WidgetPlacement::BelowEditor);
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "setWidget");
    assert_eq!(req["widgetKey"], "todo");
    assert!(
        req.get("widgetLines").is_none(),
        "a widget removal omits widgetLines rather than sending null: {req:?}"
    );
    assert_eq!(req["widgetPlacement"], "belowEditor", "a non-default placement IS emitted: {req:?}");

    // set_title → `{method:"setTitle", title}` (rpc-mode.ts:216-223).
    host_services.set_title("My Session");
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "setTitle");
    assert_eq!(req["title"], "My Session");

    // set_editor_text(text, is_paste=false) → `{method:"set_editor_text", text}` — snake_case method
    // name (rpc-mode.ts:234-241), unlike this test's other camelCase methods.
    host_services.set_editor_text("typed text", false);
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "set_editor_text");
    assert_eq!(req["text"], "typed text");

    // paste_editor_text (is_paste=true) collapses onto the SAME wire method as set_editor_text — Pi's
    // own `pasteToEditor(text) { this.setEditorText(text); }` (rpc-mode.ts:230-232).
    host_services.set_editor_text("pasted text", true);
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "set_editor_text");
    assert_eq!(req["text"], "pasted text");

    // The loop is still alive and responsive after all six effects (proves the drain arm never
    // blocked the select! loop or consumed a client-facing response slot).
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"after\"}\n").await.unwrap();
    let after = read_json_line(&mut client_reader).await;
    assert_eq!(after["command"], "get_state");

    drop(client_tx);
    rpc.await.unwrap();
}

/// `set-header`/`set-footer`/`set-tools-expanded` are delivered to the in-process [`UiEffect`] sink
/// (closing the "reaches no consumer at all" gap) but deliberately NEVER forwarded onto the RPC wire —
/// Pi's own RPC mode does not deliver them either ("Custom header/footer not supported in RPC mode -
/// requires TUI access", "Tool expansion not supported in RPC mode - no TUI", rpc-mode.ts:209-215,
/// 296-298). Proven by calling all three back-to-back and observing the very next wire line is still
/// the `get_state` response sent immediately after — no `extension_ui_request` slipped out ahead of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_header_footer_and_tools_expanded_effects_never_reach_the_wire() {
    use cyrup_ext::host::HostServices;
    use tokio::io::AsyncWriteExt;

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let session = runtime.session().await;
    let host_services = session.services().host_services.clone();

    let (mut client_tx, mut client_reader, rpc) = spawn_rpc_duplex(runtime);

    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"boot\"}\n").await.unwrap();
    let boot = read_json_line(&mut client_reader).await;
    assert_eq!(boot["command"], "get_state");

    host_services.set_header("custom header");
    host_services.set_footer("custom footer");
    host_services.set_tools_expanded(true);

    // SEAM-030 (b) — the 50 ms sleep that used to sit here is replaced by a POSITIVE
    // synchronisation point: the `get_state` round-trip below is itself the barrier. Its response
    // cannot be written until the loop has processed everything queued ahead of it, so "the very
    // next wire line is the get_state response" proves no `extension_ui_request` was emitted
    // WITHOUT depending on how long a sleep happens to be. (This one was a smell rather than a
    // defect — `extension_ui_effect_json` returns `None` for `SetHeader`/`SetFooter`/
    // `SetToolsExpanded`, so no request can ever be written regardless — but the sleep asserted
    // nothing and cost 50 ms per run.)
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"after\"}\n").await.unwrap();
    let after = read_json_line(&mut client_reader).await;
    assert_eq!(
        after["command"], "get_state",
        "the very next wire line must be the get_state response, not a stray extension_ui_request \
         for set_header/set_footer/set_tools_expanded: {after:?}"
    );
    assert_eq!(after["id"], "after");

    drop(client_tx);
    rpc.await.unwrap();
}

/// SEAM-028/SEAM-011 — pi's `setWidget` union member is
/// `{ method: "setWidget"; widgetKey: string; widgetLines: string[] | undefined; widgetPlacement?:
/// "aboveEditor" | "belowEditor" }` (`modes/rpc/rpc-types.ts:264-271` @v0.83.0). The whole
/// `RpcExtensionUIRequest` union carries **no** `widget` key on any member.
///
/// cyrup emitted a cyrup-invented `{"widget": <blob>}` instead, because both `wit/world.wit` copies
/// declared `set-widget: func(widget-json: string)` — one opaque payload where pi has three typed
/// fields. An RPC client written to pi's contract could not render extension widgets at all: no key
/// to key on, no lines to draw, no placement.
///
/// This case was `#[ignore]`d against SEAM-011 and written in pi's shape so it would go green the
/// moment the WIT widened. It has, so it runs.
#[tokio::test]
async fn set_widget_carries_pis_three_fields_and_no_widget_blob() {
    let effect = cyrup_session_svc::UiEffect::SetWidget {
        widget: serde_json::json!({"key": "text", "lines": ["hi"], "placement": "aboveEditor"}),
    };
    let req = crate::rpc::extension_ui_effect_json(&effect).expect("setWidget reaches the wire");
    assert_eq!(req["method"], "setWidget");
    assert_eq!(req["widgetKey"], "text");
    assert_eq!(req["widgetLines"], serde_json::json!(["hi"]));
    assert!(
        req.get("widget").is_none(),
        "pi's setWidget union member carries no `widget` key: {req:?}"
    );
}
