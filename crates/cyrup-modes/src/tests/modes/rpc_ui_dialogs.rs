//! The correlated half of the extension-UI transport: a guest's blocking `ui.{select,confirm,input}`
//! call leaves as an `extension_ui_request` and is resumed by the client's `extension_ui_response`.
//! Covers the happy round-trip, the envelopes pi SWALLOWS rather than answers, the host-armed
//! timeout that settles an abandoned dialog, and abort's non-interference with a pending one.

use std::sync::Arc;
use std::time::Duration;

use super::support::{build_runtime, fixture, read_json_line, spawn_rpc_duplex};
use cyrup_provider::faux::FauxProvider;

/// Mode #4 end-to-end: a loaded guest's synchronous `ui.{select,confirm,input}` capability round-trips
/// through the RPC transport — the loop emits an `extension_ui_request` on stdout and the client's
/// `extension_ui_response` resumes the wasm-suspended guest (Pi `createExtensionUIContext` +
/// `handleInputLine`, rpc-mode.ts:135-160,739-753). Multi-thread so the guest's `block_in_place`
/// reply-wait is legal. The transport is an in-memory duplex pair standing in for real stdio.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_extension_ui_request_response_round_trips() {
    use cyrup_ext::host::{DialogOptions, HostServices};
    use tokio::io::AsyncWriteExt;

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    // The SAME session Arc the loop installs its ui sink onto (shared through the runtime).
    let session = runtime.session().await;
    let host_services = session.services().host_services.clone();

    let (mut client_tx, mut client_reader, rpc) = spawn_rpc_duplex(runtime);

    // A get_state first: its response proves the loop is up and the ui sink is installed (set before
    // the select! loop), so the following guest dialog cannot race the sink.
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"boot\"}\n").await.unwrap();
    let boot = read_json_line(&mut client_reader).await;
    assert_eq!(boot["command"], "get_state");

    // (1) select → the loop emits the Pi request; the client answers with the chosen STRING, which
    //     reaches the guest UNCHANGED — the WIT `select` return is now the chosen STRING itself
    //     (world.wit:259), byte-for-byte Pi's `select(...): Promise<string|undefined>` (types.ts:127),
    //     with NO index translation anywhere in the round-trip.
    let hs = host_services.clone();
    let guest_select = tokio::spawn(async move {
        hs.select("Pick one", &serde_json::json!(["alpha", "beta", "gamma"]), &DialogOptions::default())
    });
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["type"], "extension_ui_request");
    assert_eq!(req["method"], "select");
    assert_eq!(req["title"], "Pick one");
    assert_eq!(req["options"], serde_json::json!(["alpha", "beta", "gamma"]));
    let id = req["id"].as_str().unwrap().to_string();
    client_tx
        .write_all(format!("{{\"type\":\"extension_ui_response\",\"id\":\"{id}\",\"value\":\"gamma\"}}\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(
        guest_select.await.unwrap().as_deref(),
        Some("gamma"),
        "select's wire {{value}} string passes straight through, with no index math"
    );

    // (2) confirm → `{confirmed:true}` resumes the guest with true. L4 review §2.6: the guest's
    //     `message` (a large formatted body, distinct from the `title`) reaches the wire verbatim,
    //     not hard-coded to `""`.
    let hs = host_services.clone();
    let guest_confirm = tokio::spawn(async move {
        hs.confirm("Proceed?", "a large formatted body, distinct from the title", &DialogOptions::default())
    });
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "confirm");
    assert_eq!(req["title"], "Proceed?");
    assert_eq!(
        req["message"], "a large formatted body, distinct from the title",
        "confirm's message body reaches the wire distinct from title, not hard-coded empty"
    );
    let id = req["id"].as_str().unwrap().to_string();
    client_tx
        .write_all(format!("{{\"type\":\"extension_ui_response\",\"id\":\"{id}\",\"confirmed\":true}}\n").as_bytes())
        .await
        .unwrap();
    assert!(guest_confirm.await.unwrap(), "confirm round-trips true");

    // (3) input cancelled → `{cancelled:true}` yields None (Pi `parseResponse` default). L4 review
    //     §2.7: the guest's `placeholder` reaches the wire (present, not dropped).
    let hs = host_services.clone();
    let guest_input = tokio::spawn(async move {
        hs.input("Name?", Some("e.g. Ada Lovelace"), &DialogOptions::default())
    });
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "input");
    assert_eq!(
        req["placeholder"], "e.g. Ada Lovelace",
        "input's placeholder reaches the wire instead of being dropped"
    );
    let id = req["id"].as_str().unwrap().to_string();
    client_tx
        .write_all(format!("{{\"type\":\"extension_ui_response\",\"id\":\"{id}\",\"cancelled\":true}}\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(guest_input.await.unwrap(), None, "cancelled input -> None");

    // EOF → the loop drains and returns.
    drop(client_tx);
    rpc.await.unwrap();
}

/// SEAM-086 — a malformed `extension_ui_response` is SWALLOWED, never answered.
///
/// pi's `handleInputLine` intercepts on the `type` discriminant alone and `return`s unconditionally
/// (`packages/coding-agent/src/modes/rpc/rpc-mode.ts:763-777` @v0.83.0):
///
/// ```ts
/// if (… parsed.type === "extension_ui_response") {
///     const response = parsed as RpcExtensionUIResponse;
///     const pending = pendingExtensionRequests.get(response.id);
///     if (pending) { pendingExtensionRequests.delete(response.id); pending.resolve(response); }
///     return;
/// }
/// ```
///
/// so an envelope with no `id`, a non-string `id`, or an `id` matching nothing produces **no output
/// line at all**. cyrup decided the intercept on the id instead, so those three lines fell through
/// to `dispatch` and were answered with an `Unknown command: extension_ui_response` error response —
/// an extra stdout line a client can observe. RED before the fix on the first of the three cases.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_malformed_extension_ui_response_is_swallowed_not_answered() {
    use tokio::io::AsyncWriteExt;

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let (mut client_tx, mut client_reader, rpc) = spawn_rpc_duplex(runtime);

    // Three malformed/unmatched envelopes, each of which pi swallows: no `id` at all, a non-string
    // `id`, and a well-formed string `id` that matches no pending dialog.
    for line in [
        "{\"type\":\"extension_ui_response\",\"value\":\"x\"}\n",
        "{\"type\":\"extension_ui_response\",\"id\":42,\"value\":\"x\"}\n",
        "{\"type\":\"extension_ui_response\",\"id\":\"ext-ui-999\",\"value\":\"x\"}\n",
    ] {
        client_tx.write_all(line.as_bytes()).await.unwrap();
    }

    // A sentinel command AFTER them. Because the loop services stdin lines in order, the first line
    // the client reads back is the sentinel's response iff none of the three produced output.
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"sentinel\"}\n").await.unwrap();
    let first = read_json_line(&mut client_reader).await;
    assert_eq!(
        first["command"], "get_state",
        "a malformed extension_ui_response must produce NO stdout line; got {first} first"
    );
    assert_eq!(first["id"], "sentinel");

    drop(client_tx);
    rpc.await.unwrap();
}

/// L4 review §2.2 (CRITICAL): a `timeout_ms`-bearing dialog whose RPC client NEVER answers must still
/// resolve within that window — Pi's `createDialogPromise` host-armed `setTimeout`
/// (`rpc-mode.ts:114-119`) ALWAYS settles the Promise regardless of client behavior. Proves the fix
/// end-to-end over the real wire protocol: the client sees the `timeout` field on the outgoing request
/// (rpc-types.ts shape) but deliberately never answers it, and the loop stays alive/responsive
/// afterward (a subsequent `get_state` still gets served — the turn was never left hung).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_extension_ui_request_times_out_to_the_default_when_client_never_responds() {
    use cyrup_ext::host::{DialogOptions, HostServices};
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

    // Open a confirm dialog with a short live countdown; the guest call is driven on a blocking task
    // exactly as the wasm-suspended host import would be.
    let hs = host_services.clone();
    let opts = DialogOptions { timeout_ms: Some(80), signal_id: None };
    let guest_confirm = tokio::spawn(async move { hs.confirm("Proceed?", "body", &opts) });

    // The client sees the request, including Pi's `timeout` field — and simply never answers it.
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "confirm");
    assert_eq!(req["timeout"], 80, "the wire `timeout` field carries opts.timeout_ms verbatim");

    // The guest call must settle to the confirm default (`false`) on its own, well inside a generous
    // bound — proving the host, not the client, is what unblocks it.
    let resolved = tokio::time::timeout(Duration::from_secs(5), guest_confirm)
        .await
        .expect("the dialog must not hang past its timeout_ms")
        .expect("confirm task");
    assert!(!resolved, "an unanswered confirm settles to Pi's `false` default on timeout");
    // SEAM-030 — the `started.elapsed() < 2s` margin that used to follow is DELETED: it carried no
    // semantic content the `timeout(5s)` + `assert!(!resolved)` above does not already carry (the
    // dialog demonstrably settled on its own, unanswered), and it was the most flake-prone
    // assertion in the whole modes suite.

    // The loop is still alive and serving requests — the abandoned dialog never hung the session.
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"after\"}\n").await.unwrap();
    let after = read_json_line(&mut client_reader).await;
    assert_eq!(after["command"], "get_state");
    assert_eq!(after["id"], "after");

    drop(client_tx);
    rpc.await.unwrap();
}

/// `abort`/`abort_retry` must NOT force-dismiss an open `confirm`/`input`/`select` dialog. Pi's
/// `session.abort()` (`agent-session.ts`) only cancels the run; `rpc-mode.ts`'s `case "abort"`
/// (~line 424) and `case "abort_retry"` (~line 541) never touch `pendingExtensionRequests` — a
/// dialog is dismissed early ONLY through the extension's own opt-in `signal` binding
/// (`ExtensionUIDialogOptions.signal`, types.ts:320-321), which nothing in Pi's first-party code
/// wires to "the turn got aborted" by default. The client here deliberately never sends an
/// `extension_ui_response` while aborting — the still-open dialog must remain genuinely pending
/// through the abort, and only settle once a real response finally arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_abort_does_not_force_resolve_a_pending_dialog() {
    use cyrup_ext::host::{DialogOptions, HostServices};
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

    // Open a `select` dialog with NO timeout at all — nothing but a genuine response can unblock it.
    let hs = host_services.clone();
    let mut guest_select =
        tokio::spawn(async move {
            hs.select("Pick one", &serde_json::json!(["a", "b"]), &DialogOptions::default())
        });
    let req = read_json_line(&mut client_reader).await;
    assert_eq!(req["method"], "select");
    let dialog_id = req["id"].as_str().unwrap().to_string();

    // The client never answers the dialog — it aborts the turn instead.
    client_tx.write_all(b"{\"type\":\"abort\",\"id\":\"stop\"}\n").await.unwrap();
    let abort_resp = read_json_line(&mut client_reader).await;
    assert_eq!(abort_resp["command"], "abort");
    assert_eq!(abort_resp["success"], true);

    // The guest's dialog must NOT resolve from the abort alone — it stays genuinely pending.
    let still_pending = tokio::time::timeout(Duration::from_millis(300), &mut guest_select).await;
    assert!(
        still_pending.is_err(),
        "abort must not force-resolve an open dialog: {still_pending:?}"
    );

    // The loop is still alive, and the dialog is STILL answerable by a real `extension_ui_response`
    // after the abort — proving it was left genuinely pending, not silently dropped.
    client_tx.write_all(b"{\"type\":\"get_state\",\"id\":\"after\"}\n").await.unwrap();
    let after = read_json_line(&mut client_reader).await;
    assert_eq!(after["command"], "get_state");

    client_tx
        .write_all(format!("{{\"type\":\"extension_ui_response\",\"id\":\"{dialog_id}\",\"value\":\"a\"}}\n").as_bytes())
        .await
        .unwrap();
    let resolved = tokio::time::timeout(Duration::from_secs(2), guest_select)
        .await
        .expect("the dialog must still be answerable after an unrelated abort")
        .expect("select task");
    assert_eq!(resolved.as_deref(), Some("a"), "a real response after abort still resumes the guest");

    drop(client_tx);
    rpc.await.unwrap();
}
