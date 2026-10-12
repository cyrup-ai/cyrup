//! SEAM-162 — the `prompt`, `steer` and `follow_up` responses carry pi's `{ disposition }`.
//!
//! pi (`modes/rpc/rpc-mode.ts` @f1b2e77f5, identical at v1.0.4): `case "prompt"` writes
//! `output(success(id, "prompt", { disposition }))` from `preflightResult` (`:394-414`, the `output` at `:405`);
//! `case "steer"` / `case "follow_up"` return `success(id, "steer", { disposition })` /
//! `success(id, "follow_up", { disposition })` (`:418`, `:423`). The values are pi's
//! `PromptDisposition = QueuedInputDisposition | "started"` and
//! `QueuedInputDisposition = "handled" | "queued"` (`core/agent-session.ts:310-311`).
//!
//! Every wire string is asserted literally on the bytes the host wrote. Seven combinations exist:
//! `prompt` answers `started`, `queued` or `handled`; `steer` and `follow_up` answer `queued` or
//! `handled`. `started` is not reachable for `steer`/`follow_up` — pi's `_queueUserInput`
//! (`agent-session.ts:2173-2200`) and cyrup's `queue_user_input` only queue or report handled;
//! neither starts a run — so those two combinations have no test.

use std::sync::Arc;

use super::support::{
    build_runtime, build_runtime_with_ext, fixture, read_json_line, spawn_rpc_duplex,
};
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    EventKind, ExtError, HandledValue, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use serde_json::Value;
use tokio::io::{AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::Notify;

/// Write one command line and read until the response to `id`, skipping events.
async fn ask(
    tx: &mut DuplexStream,
    rx: &mut BufReader<DuplexStream>,
    line: &str,
    id: &str,
) -> Value {
    tx.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    tx.flush().await.unwrap();
    loop {
        let v = read_json_line(rx).await;
        if v["type"] == "response" && v["id"] == id {
            return v;
        }
    }
}

/// The response's `data`, which must be exactly `{ "disposition": <wire> }`.
fn assert_disposition(response: &Value, command: &str, wire: &str) {
    assert_eq!(response["command"], command, "{response}");
    assert_eq!(response["success"], true, "{response}");
    assert_eq!(
        response["data"],
        serde_json::json!({ "disposition": wire }),
        "pi's {command} response data is {{ disposition: \"{wire}\" }}: {response}"
    );
}

/// `started` — a `prompt` on an idle session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_on_an_idle_session_answers_started() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let runtime = build_runtime(&fx, faux).await;
    let (mut tx, mut rx, rpc) = spawn_rpc_duplex(runtime);

    let r = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"prompt","id":"p","message":"hi"}"#,
        "p",
    )
    .await;
    assert_disposition(&r, "prompt", "started");

    loop {
        if read_json_line(&mut rx).await["type"] == "agent_settled" {
            break;
        }
    }
    drop(tx);
    rpc.await.expect("rpc loop exits at EOF");
}

/// `queued` — during a run: a `prompt` with `streamingBehavior` (both behaviours), a `steer` and a
/// `follow_up`. The first provider call parks until released, so the run is provably live while
/// the four commands are answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn during_a_run_prompt_steer_and_follow_up_answer_queued() {
    let fx = fixture();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let faux = Arc::new(FauxProvider::new());
    let mut steps = vec![FauxResponseStep::async_factory({
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        move |_, _, _, _| {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            async move {
                entered.notify_one();
                release.notified().await;
                faux_assistant_message(vec![faux_text("first")], StopReason::Stop)
            }
        }
    })];
    // The queued messages continue the run once it is released.
    for _ in 0..6 {
        steps.push(faux_assistant_message(vec![faux_text("more")], StopReason::Stop).into());
    }
    faux.set_response_steps(steps);
    let runtime = build_runtime(&fx, faux).await;
    let (mut tx, mut rx, rpc) = spawn_rpc_duplex(runtime);

    let r = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"prompt","id":"p","message":"go"}"#,
        "p",
    )
    .await;
    assert_disposition(&r, "prompt", "started");
    entered.notified().await;

    let r = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"prompt","id":"pf","message":"later","streamingBehavior":"followUp"}"#,
        "pf",
    )
    .await;
    assert_disposition(&r, "prompt", "queued");
    let r = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"prompt","id":"ps","message":"now","streamingBehavior":"steer"}"#,
        "ps",
    )
    .await;
    assert_disposition(&r, "prompt", "queued");
    let r = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"steer","id":"s","message":"turn"}"#,
        "s",
    )
    .await;
    assert_disposition(&r, "steer", "queued");
    let r = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"follow_up","id":"f","message":"then"}"#,
        "f",
    )
    .await;
    assert_disposition(&r, "follow_up", "queued");

    release.notify_one();
    loop {
        if read_json_line(&mut rx).await["type"] == "agent_settled" {
            break;
        }
    }
    drop(tx);
    rpc.await.expect("rpc loop exits at EOF");
}

/// An `input` handler that services every submission (pi `{ action: "handled" }`).
struct ServicesEveryInput;

#[async_trait::async_trait]
impl NativeExtension for ServicesEveryInput {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("services-every-input")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::Input]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::Input { .. } => HookOutcome::Handled(HandledValue(serde_json::json!({
                "action": "handled"
            }))),
            _ => HookOutcome::Noop,
        }
    }
}

/// `handled` — a `prompt`, a `steer` and a `follow_up` an `input` handler serviced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_steer_and_follow_up_an_input_handler_serviced_answer_handled() {
    let fx = fixture();
    let runtime = build_runtime_with_ext(
        &fx,
        Arc::new(FauxProvider::new()),
        Arc::new(ServicesEveryInput),
    )
    .await;
    let (mut tx, mut rx, rpc) = spawn_rpc_duplex(runtime);

    let r = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"prompt","id":"p","message":"hi"}"#,
        "p",
    )
    .await;
    assert_disposition(&r, "prompt", "handled");
    let r = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"steer","id":"s","message":"turn"}"#,
        "s",
    )
    .await;
    assert_disposition(&r, "steer", "handled");
    let r = ask(
        &mut tx,
        &mut rx,
        r#"{"type":"follow_up","id":"f","message":"then"}"#,
        "f",
    )
    .await;
    assert_disposition(&r, "follow_up", "handled");

    drop(tx);
    rpc.await.expect("rpc loop exits at EOF");
}

/// The client reads the same answers back, typed (pi `rpc-client.ts:198-221`), and sends
/// `streamingBehavior` with a prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_returns_the_disposition() {
    use crate::{PromptDisposition, QueuedInputDisposition, RpcClient};

    let fx = fixture();
    let runtime = build_runtime_with_ext(
        &fx,
        Arc::new(FauxProvider::new()),
        Arc::new(ServicesEveryInput),
    )
    .await;
    let (tx, reader, rpc) = spawn_rpc_duplex(runtime);
    let client = RpcClient::attach(reader, tx);

    assert_eq!(
        client
            .prompt(
                "hi",
                None,
                Some(cyrup_session_svc::StreamingBehavior::FollowUp)
            )
            .await
            .unwrap(),
        PromptDisposition::Handled
    );
    assert_eq!(
        client.steer("turn", None).await.unwrap(),
        QueuedInputDisposition::Handled
    );
    assert_eq!(
        client.follow_up("then", None).await.unwrap(),
        QueuedInputDisposition::Handled
    );

    client.stop().await;
    rpc.await.expect("rpc loop exits at EOF");
}

/// The three wire strings, one per variant, both directions.
#[test]
fn the_disposition_wire_strings_are_pi_s() {
    use crate::{PromptDisposition, QueuedInputDisposition};
    for (d, wire) in [
        (PromptDisposition::Started, "started"),
        (PromptDisposition::Queued, "queued"),
        (PromptDisposition::Handled, "handled"),
    ] {
        assert_eq!(serde_json::to_value(d).unwrap(), Value::from(wire));
        assert_eq!(
            serde_json::from_value::<PromptDisposition>(Value::from(wire)).unwrap(),
            d
        );
    }
    for (d, wire) in [
        (QueuedInputDisposition::Queued, "queued"),
        (QueuedInputDisposition::Handled, "handled"),
    ] {
        assert_eq!(serde_json::to_value(d).unwrap(), Value::from(wire));
    }
    assert!(
        serde_json::from_value::<QueuedInputDisposition>(Value::from("started")).is_err(),
        "pi's QueuedInputDisposition has no \"started\""
    );
}
