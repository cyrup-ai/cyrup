//! NATIVE SLASH-COMMAND OUTPUT REACHES THE USER — end-to-end proof for the `Ok(Some(_))` arm of
//! `AgentSession::try_execute_extension_command` (session.rs:958-1013).
//!
//! Pi's `_tryExecuteExtensionCommand` (`coding-agent/src/core/agent-session.ts:1278-1301`) awaits
//! `command.handler(args, ctx)` — a `Promise<void>` — so the handler's *return value* is genuinely
//! discarded there; the handler talks to the user through the `ctx` it was handed
//! (`createCommandContext`, :1288). On the error path Pi calls
//! `this._extensionRunner.emitError({ extensionPath: `command:${commandName}`, event: "command",
//! error: ... })` (:1295-1299) and STILL `return true` (:1300) — the command counts as handled and
//! the text never reaches the model as a prompt. The OBSERVABLE contract is therefore: running a
//! registered slash command shows the user its response, and a throwing handler shows the user an
//! error, and in neither case is `/name ...` treated as a prompt.
//!
//! cyrup's native seam differs in MECHANISM only: `NativeExtension::execute_command` returns
//! `Result<Option<String>, ExtError>` (`cyrup-ext/src/native.rs:338-345`) instead of writing to a
//! ctx, because a Rust `Arc<dyn NativeExtension>` handler cannot capture the session the way a JS
//! closure captures `this`. `ExtensionHost::execute_native_command`
//! (`cyrup-ext/src/facade.rs:316-350`) wraps it as
//! `Result<Option<Result<Option<String>, ExtError>>, ExtError>`: outer = routing, `Option` = did a
//! native extension own the name, inner = what the handler itself returned.
//!
//! THE BUG THESE TESTS PIN: the `Ok(Some(_))` arm bound that inner `Result` to `_` and threw it
//! away, so cyrup's native built-ins — which answer their slash commands EXCLUSIVELY through that
//! return value — ran and printed nothing. Asserting on `execute_native_command`'s return value
//! would prove nothing; that layer always worked. So every test here drives the REAL public entry
//! point `AgentSession::prompt("/name args")` and asserts on the CONSUMER side of the UI channel:
//! the `UiEffect::Notify` that `LiveHostServices::notify` publishes to the sink a mode attaches via
//! `set_ui_effect_sink` (`host_services.rs:419-431`, :641-645).
//!
//! Deliberately native, not wasm: `crates/cyrup-it/tests/session_svc/wasm_slash_command.rs` covers
//! the guest route and pays a nested `cargo build` for it. A native `Arc<dyn NativeExtension>` needs
//! no build step, so these run in milliseconds and add no fixture-target directory to `/tmp`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{ExtensionId, Message, StopReason};
use cyrup_ext::{
    CommandDescriptor, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use super::common::{base_config, base_config_no_extensions, fixture, Fixture};
use crate::{NotifyKind, SessionBuilder, UiEffect};
use tokio::sync::mpsc::UnboundedReceiver;

/// What a registered command's handler should answer with. One native extension registers one
/// command per variant, so a single session can exercise every shape of handler reply.
#[derive(Clone, Debug)]
enum Reply {
    /// `Ok(Some(text))` — the normal case: the handler produced user-facing output.
    Text(&'static str),
    /// `Ok(None)` — the handler deliberately says nothing (it did its work silently).
    Nothing,
    /// `Ok(Some("   "))` — whitespace-only output, which must be treated as saying nothing.
    Blank,
    /// `Err(..)` — the handler failed. Pi surfaces this via `emitError` and still reports handled.
    Fail(&'static str),
}

/// A native built-in that registers several slash commands, each with a scripted reply shape, and
/// counts how many times its `execute_command` actually ran (so a test can tell "the handler never
/// ran" apart from "the handler ran and its output was dropped" — the second is the bug).
struct ScriptedCommands {
    /// `(command name, what its handler answers)`.
    commands: Vec<(&'static str, Reply)>,
    ran: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NativeExtension for ScriptedCommands {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("scripted-commands")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for (name, _) in &self.commands {
            api.register_command(
                *name,
                CommandDescriptor {
                    description: format!("scripted command {name}"),
                    completions: Vec::new(),
                },
            );
        }
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    /// The seam under test. It answers ONLY through this return value and never touches any other
    /// output channel — exactly like `cyrup-ext-subagents`/`cyrup-intercom`, whose native command
    /// handlers contain no `notify` calls at all. If the caller drops this value, the user sees
    /// nothing.
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        self.ran.fetch_add(1, Ordering::AcqRel);
        let Some((_, reply)) = self.commands.iter().find(|(n, _)| *n == name) else {
            return Err(ExtError::Component(format!("no handler for `{name}`")));
        };
        match reply {
            // Echo the args back too, so a test can prove the payload that surfaced is THIS
            // invocation's output and not a fixed string produced somewhere else.
            Reply::Text(text) => Ok(Some(if args.is_empty() {
                (*text).to_string()
            } else {
                format!("{text} [args={args}]")
            })),
            Reply::Nothing => Ok(None),
            Reply::Blank => Ok(Some("   \n  \t ".to_string())),
            Reply::Fail(msg) => Err(ExtError::Component((*msg).to_string())),
        }
    }
}

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    // Enough scripted responses for the fall-through prompts these tests send; unused entries are
    // harmless.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
    ]);
    faux
}

fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

/// Drain every `UiEffect` currently queued on the sink, keeping only the `Notify`s. This is the
/// CONSUMER side of the channel a mode (interactive/rpc) attaches — the same one the TUI renders.
fn drain_notifies(rx: &mut UnboundedReceiver<UiEffect>) -> Vec<(String, NotifyKind)> {
    let mut out = Vec::new();
    while let Ok(effect) = rx.try_recv() {
        if let UiEffect::Notify { message, kind } = effect {
            out.push((message, kind));
        }
    }
    out
}

/// Build a live session with the scripted native extension registered, and attach a ui-effect sink
/// so the test can observe what a real mode would render.
async fn session_with(
    fx: &Fixture,
    commands: Vec<(&'static str, Reply)>,
) -> (crate::AgentSession, UnboundedReceiver<UiEffect>, Arc<AtomicUsize>) {
    let ran = Arc::new(AtomicUsize::new(0));
    let ext = Arc::new(ScriptedCommands { commands, ran: ran.clone() });
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config_no_extensions(fx))
        .with_native_extension(ext)
        .build()
        .await
        .unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    session.services().host_services.set_ui_effect_sink(tx);
    (session, rx, ran)
}

/// THE HEADLINE PROOF. A NATIVE extension registers `/report`; `prompt("/report weekly")` drives the
/// real `prepare` → `try_execute_extension_command` → `execute_native_command` path, and the text
/// the handler returned must arrive on the UI channel as an Info `UiEffect::Notify`.
///
/// Before the fix this test fails on the notify assertion while `ran == 1` — proving the handler
/// executed and its payload was thrown away, which is precisely the defect (every native built-in
/// command ran silently).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_command_output_reaches_the_ui_channel() {
    let fx = fixture();
    let (session, mut ui_rx, ran) =
        session_with(&fx, vec![("report", Reply::Text("REPORT-OUTPUT"))]).await;

    // The native-registered command is in the host registry (the routing precondition).
    assert!(
        session
            .services()
            .ext_host
            .registry()
            .command_names()
            .unwrap()
            .iter()
            .any(|n| n == "report"),
        "the native-registered `/report` command is in the host command registry"
    );
    assert_eq!(ran.load(Ordering::Acquire), 0, "the handler has not run before the prompt");

    // Drive the REAL public entry point.
    let _ = session.prompt("/report weekly").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(ran.load(Ordering::Acquire), 1, "the native command handler ran exactly once");

    // ---- THE ASSERTION THE BUG BROKE: the handler's payload reached the user. ----
    let notifies = drain_notifies(&mut ui_rx);
    assert_eq!(
        notifies.len(),
        1,
        "exactly one notification surfaced for one command (no double-print, no silence): {notifies:?}"
    );
    assert_eq!(
        notifies[0].0, "REPORT-OUTPUT [args=weekly]",
        "the notification carries THIS invocation's handler output verbatim (args included), not a \
         generic message: {notifies:?}"
    );
    assert_eq!(
        notifies[0].1,
        NotifyKind::Info,
        "a successful command's output surfaces as Info, not an error: {notifies:?}"
    );

    // Still short-circuited: `/report weekly` never became a prompt (Pi returns `true`,
    // agent-session.ts:1300).
    assert!(
        user_texts(&session.messages().await).iter().all(|t| !t.contains("/report")),
        "the native slash command was consumed — no user message went to the model"
    );
}

/// MIRROR CASE. A handler that deliberately says nothing must stay silent: the fix must not
/// manufacture a notification out of `Ok(None)`, nor out of whitespace-only output. Without this
/// guard the natural "always notify" implementation would spam an empty popup for every silent
/// built-in command.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_native_command_emits_no_notification() {
    let fx = fixture();
    let (session, mut ui_rx, ran) = session_with(
        &fx,
        vec![("quiet", Reply::Nothing), ("blank", Reply::Blank), ("loud", Reply::Text("SPEAKS"))],
    )
    .await;

    // `Ok(None)`: the handler ran, said nothing, and nothing is shown.
    let _ = session.prompt("/quiet").await.unwrap();
    session.wait_for_idle().await;
    assert_eq!(ran.load(Ordering::Acquire), 1, "the `/quiet` handler ran");
    assert!(
        drain_notifies(&mut ui_rx).is_empty(),
        "a handler returning Ok(None) produces NO notification"
    );

    // `Ok(Some(whitespace))`: same — trimmed-empty output is "said nothing", not an empty popup.
    let _ = session.prompt("/blank").await.unwrap();
    session.wait_for_idle().await;
    assert_eq!(ran.load(Ordering::Acquire), 2, "the `/blank` handler ran");
    assert!(
        drain_notifies(&mut ui_rx).is_empty(),
        "a handler returning whitespace-only output produces NO notification"
    );

    // Control: the sink IS live — a speaking command on the SAME session does notify. Without this
    // the two assertions above would also pass with the ui channel simply broken.
    let _ = session.prompt("/loud").await.unwrap();
    session.wait_for_idle().await;
    let notifies = drain_notifies(&mut ui_rx);
    assert_eq!(notifies.len(), 1, "the control command surfaced its output: {notifies:?}");
    assert_eq!(notifies[0].0, "SPEAKS", "control output verbatim: {notifies:?}");

    // Silent or not, all three were consumed as commands, never sent as prompts.
    let texts = user_texts(&session.messages().await);
    assert!(
        texts.iter().all(|t| !t.contains("/quiet") && !t.contains("/blank") && !t.contains("/loud")),
        "silent commands are still fully handled — none fell through to the model: {texts:?}"
    );
}

/// ERROR CASE. A failing handler must (a) surface as an ERROR notification carrying Pi's
/// `command:<name>` attribution (`agent-session.ts:1295-1299`), and (b) still count as HANDLED —
/// Pi `return true` at :1300 — so a broken command never leaks `/name ...` to the model as a prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failing_native_command_surfaces_an_error_and_still_counts_as_handled() {
    let fx = fixture();
    let (session, mut ui_rx, ran) =
        session_with(&fx, vec![("boom", Reply::Fail("handler exploded"))]).await;

    let _ = session.prompt("/boom now").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(ran.load(Ordering::Acquire), 1, "the failing handler actually ran");

    // (a) the failure is visible, as an Error, attributed to the command.
    let notifies = drain_notifies(&mut ui_rx);
    assert_eq!(notifies.len(), 1, "the handler error surfaced exactly once: {notifies:?}");
    assert_eq!(
        notifies[0].1,
        NotifyKind::Error,
        "a handler failure surfaces as Error, not Info: {notifies:?}"
    );
    assert!(
        notifies[0].0.starts_with("command:boom: "),
        "the error carries Pi's `command:<name>` attribution (agent-session.ts:1296): {notifies:?}"
    );
    assert!(
        notifies[0].0.contains("handler exploded"),
        "the error carries the handler's own message: {notifies:?}"
    );

    // (b) STILL handled: the command did not fall through to being treated as a prompt.
    let texts = user_texts(&session.messages().await);
    assert!(
        texts.iter().all(|t| !t.contains("/boom")),
        "a failing command is still fully handled — it never reaches the model as a prompt: {texts:?}"
    );

    // Contrast: an UNREGISTERED `/name` genuinely does fall through (Pi `getCommand` → undefined ⇒
    // `return false`, agent-session.ts:1284-1285). This is what makes the assertion above meaningful rather
    // than vacuous — it pins the difference between "handled with an error" and "not handled".
    let _ = session.prompt("/nosuchcommand please").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        user_texts(&session.messages().await).iter().any(|t| t.contains("/nosuchcommand please")),
        "an unregistered slash command still falls through to normal prompt handling"
    );
    assert_eq!(
        ran.load(Ordering::Acquire),
        1,
        "the unregistered name never reached the extension's handler"
    );
}

/// REGRESSION FENCE for the surfacing not being one-shot or order-scrambled: several commands in a
/// row each surface their OWN output, once, in order. A caching or take-once implementation of the
/// fix would pass the headline test and fail this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_invocation_surfaces_its_own_output_in_order() {
    let fx = fixture();
    let (session, mut ui_rx, ran) = session_with(
        &fx,
        vec![("alpha", Reply::Text("A")), ("beta", Reply::Text("B")), ("gamma", Reply::Nothing)],
    )
    .await;

    for cmd in ["/alpha 1", "/beta 2", "/gamma 3", "/alpha 4"] {
        let _ = session.prompt(cmd).await.unwrap();
        session.wait_for_idle().await;
    }
    assert_eq!(ran.load(Ordering::Acquire), 4, "all four invocations reached the handler");

    let notifies = drain_notifies(&mut ui_rx);
    let messages: Vec<String> = notifies.iter().map(|(m, _)| m.clone()).collect();
    assert_eq!(
        messages,
        vec![
            "A [args=1]".to_string(),
            "B [args=2]".to_string(),
            // `/gamma` is silent — it contributes no entry at all, which is what keeps this a
            // 3-element sequence rather than a 4-element one with a blank in the middle.
            "A [args=4]".to_string(),
        ],
        "each speaking invocation surfaced its own output exactly once, in submission order"
    );
    assert!(
        notifies.iter().all(|(_, k)| *k == NotifyKind::Info),
        "every successful command surfaced as Info: {notifies:?}"
    );
}

/// The headless contract is preserved: with NO ui-effect sink attached (print/json mode, Pi's
/// `noOpUIContext` — `runner.ts:234-244`), surfacing the payload must silently drop rather than
/// block, error, or panic — and the command must still be handled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_session_without_a_ui_sink_still_handles_the_command() {
    let fx = fixture();
    let ran = Arc::new(AtomicUsize::new(0));
    let ext = Arc::new(ScriptedCommands {
        commands: vec![("report", Reply::Text("REPORT-OUTPUT"))],
        ran: ran.clone(),
    });
    // NOTE: no `set_ui_effect_sink` — this is the headless shape.
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config_no_extensions(&fx))
        .with_native_extension(ext)
        .build()
        .await
        .unwrap();

    let _ = session.prompt("/report weekly").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(ran.load(Ordering::Acquire), 1, "the handler ran under a headless session");
    assert!(
        user_texts(&session.messages().await).iter().all(|t| !t.contains("/report")),
        "the command is still consumed with no ui sink attached"
    );
}

/// Belt-and-braces on the seam's own contract, so a future refactor cannot "fix" the tests above by
/// changing what `execute_native_command` returns: the handler payload really is available at that
/// layer. This is the LOWER bound the headline test sits on top of — it passed even WITH the bug,
/// which is exactly why it is not sufficient on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_facade_layer_does_carry_the_payload() {
    let fx = fixture();
    let (session, _ui_rx, _ran) =
        session_with(&fx, vec![("report", Reply::Text("REPORT-OUTPUT"))]).await;

    let cancel = cyrup_core::CancelToken::new();
    let routed = tokio::time::timeout(
        Duration::from_secs(10),
        session.services().ext_host.execute_native_command("report", "weekly", &cancel),
    )
    .await
    .expect("execute_native_command settled")
    .expect("routing succeeded");

    let handler_result = routed.expect("a native extension owned the command name");
    assert_eq!(
        handler_result.expect("the handler returned Ok").as_deref(),
        Some("REPORT-OUTPUT [args=weekly]"),
        "the facade hands the caller the handler's payload — dropping it is the caller's bug"
    );
}

// ==================== the same dispatch seen from the PROMPT side: it short-circuits ====

/// A native extension registering a `/greet` command whose handler records its args + returns text.
struct GreetCommand(Arc<Mutex<Vec<String>>>);
#[async_trait::async_trait]
impl NativeExtension for GreetCommand {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("greet-command")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_command(
            "greet",
            CommandDescriptor { description: "greet someone".into(), completions: vec![] },
        );
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    async fn execute_command(
        &self,
        _name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        // A command runs command-tier (session mutation allowed): the deadlock guard must pass.
        ctx.require_command_tier()?;
        self.0.lock().unwrap().push(args.to_string());
        Ok(Some(format!("hello {args}")))
    }
}

/// gap-09 #13 (native residue) — slash `_tryExecuteExtensionCommand` *exec* dispatch for NATIVE
/// extensions (Pi agent-session.ts:1004-1013,1148-1172): a `/<cmd>` matching a registered native
/// command runs its command-tier handler and short-circuits — no user message is sent/persisted
/// (Pi's `_tryExecuteExtensionCommand` returns `true` -> the prompt is consumed).
#[tokio::test]
async fn native_slash_command_executes_and_short_circuits_the_prompt() {
    let fx = fixture();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(GreetCommand(calls.clone())))
        .build()
        .await
        .unwrap();

    let _ = session.prompt("/greet world").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(
        calls.lock().unwrap().clone(),
        vec!["world".to_string()],
        "the native command handler ran with the parsed args"
    );
    assert!(
        user_texts(&session.messages().await).iter().all(|t| !t.contains("/greet")),
        "the slash command was consumed — no user message was sent to the model"
    );

    // A `/unknown` command (no native owner) is NOT consumed: it falls through to a normal prompt.
    let _ = session.prompt("/unknown stuff").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        user_texts(&session.messages().await).iter().any(|t| t.contains("/unknown stuff")),
        "an unmatched slash command falls through to normal prompt handling"
    );
}
