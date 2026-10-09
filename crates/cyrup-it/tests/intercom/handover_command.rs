//! ICOM-078 / ICOM-085 — `/handover`, the handover picker, and the LIVE `/intercom` session list
//! (with `h` and `alt+m`), driven through the production `NativeExtension` entry points against a
//! real `cyrup-intercom-broker` and a connected peer.
//!
//! Upstream (`pi-intercom` `v0.16.1`): `runHandoverCommand` (`index.ts:3023-3041`),
//! `openHandoverPicker` (`:3043-3095`), `performHandover` (`:3097-3147`), `openIntercomOverlay`
//! (`:3149-3220`), `registerCommand("handover")` (`:3238-3241`) and `registerShortcut("alt+m")`
//! (`:3243-3246`), over `ui/handover-picker.ts` and `ui/session-list.ts`.
//!
//! The host is [`OverlayHost`]: a `HostServices` double whose `open_overlay` drives the overlay it
//! is handed the way the TUI does — it paints frames, routes scripted keys, ticks it on its own
//! cadence and stops when the overlay closes — while `editor`, `input`, `notify`, `append_entry`
//! and the handover completion are scripted and recorded. Everything past that seam is
//! production: the broker, the peer, the delivery, the relay runner seam.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{AssistantMessage, CancelToken, Content, Message as LlmMessage, StopReason};
use cyrup_ext::host::{StandaloneCompletion, StandaloneCompletionRefusal};
use cyrup_ext::{
    DialogOptions, ExtMode, HostCtx, HostEvent, HostServices, InteractiveOverlay, NativeExtension,
    NotifyKind, OverlayKey, OverlayKeyCode, OverlayOutcome,
};
use cyrup_intercom::config::load_config;
use cyrup_intercom::cross_machine::{CommandResult, CommandRunner};
use cyrup_intercom::extension::{
    HANDOVER_COMMAND, HANDOVER_REQUIRES_TUI, INTERCOM_COMMAND, INTERCOM_SHORTCUT, IntercomExtension,
};
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient};
use cyrup_intercom::transport::spawn::wait_for_broker;
use cyrup_provider::faux::{faux_assistant_message, faux_text};
use futures::future::BoxFuture;

use crate::common::{registration, spawn_broker, within, write_broker_command};

const MY_SESSION_ID: &str = "session-a11ce000a11ce000";
const PEER_SESSION_ID: &str = "session-0e0e0e0e0e0e0e0e";
const CHILD_SESSION_ID: &str = "subagent-worker-run-1";
const SUMMARY: &str = "## Next task\nPort the schema fix.";

// ---------------------------------------------------------------------------------------------
// The overlay-driving host
// ---------------------------------------------------------------------------------------------

/// One step of a scripted overlay session.
#[derive(Clone, Debug)]
enum Step {
    /// Route one key.
    Key(OverlayKey),
    /// Type each character as its own key.
    Type(&'static str),
    /// Tick and repaint until a frame contains the text.
    WaitFor(&'static str),
    /// Tick until the overlay closes itself (`should_close`).
    UntilClosed,
}

fn key(code: OverlayKeyCode) -> Step {
    Step::Key(OverlayKey::plain(code))
}

/// What the next `open_overlay` does.
enum Script {
    /// Drive the overlay through these steps; it must be closed at the end.
    Drive(Vec<Step>),
    /// Refuse it, as a host with no interactive surface does.
    NoSurface,
}

/// The completion the handover asks for.
enum Completion {
    Reply(&'static str),
    /// Never answer; resolve only when the caller's token fires, recording that it did.
    HangUntilCancelled,
}

/// What `editor` returns, given the generated text.
type EditFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

struct OverlayHost {
    scripts: Mutex<VecDeque<Script>>,
    /// Every frame painted, one joined string per frame, tagged by overlay number.
    frames: Mutex<Vec<(usize, String)>>,
    overlays_opened: AtomicUsize,
    completion: Completion,
    completion_cancelled: Arc<Mutex<bool>>,
    requests: Mutex<Vec<StandaloneCompletion>>,
    edit: EditFn,
    editors: Mutex<Vec<(String, String)>>,
    input_answer: Option<String>,
    inputs: Mutex<Vec<(String, Option<String>)>>,
    notified: Mutex<Vec<(String, NotifyKind)>>,
    entries: Mutex<Vec<(String, serde_json::Value)>>,
}

impl OverlayHost {
    fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            frames: Mutex::new(Vec::new()),
            overlays_opened: AtomicUsize::new(0),
            completion: Completion::Reply(SUMMARY),
            completion_cancelled: Arc::new(Mutex::new(false)),
            requests: Mutex::new(Vec::new()),
            edit: Box::new(|text| Some(format!("{text}\n\nEdited by the human."))),
            editors: Mutex::new(Vec::new()),
            input_answer: None,
            inputs: Mutex::new(Vec::new()),
            notified: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
        }
    }

    fn frames_of(&self, overlay: usize) -> Vec<String> {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| *n == overlay)
            .map(|(_, f)| f.clone())
            .collect()
    }

    fn all_frames(&self) -> String {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .map(|(n, f)| format!("--- overlay {n}\n{f}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn notified(&self) -> Vec<(String, NotifyKind)> {
        self.notified.lock().unwrap().clone()
    }

    fn sent_entries(&self) -> Vec<serde_json::Value> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| t == "intercom_sent")
            .map(|(_, d)| d.clone())
            .collect()
    }

    fn paint(&self, n: usize, overlay: &mut dyn InteractiveOverlay) -> String {
        let frame: String = overlay
            .render(88, 40)
            .iter()
            .map(|line| line.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        self.frames.lock().unwrap().push((n, frame.clone()));
        frame
    }

    fn drive(&self, n: usize, overlay: &mut dyn InteractiveOverlay, steps: Vec<Step>) {
        let mut closed = false;
        let mut keys: Vec<OverlayKey> = Vec::new();
        let tick = |overlay: &mut dyn InteractiveOverlay| {
            overlay.tick();
            std::thread::sleep(Duration::from_millis(10));
        };
        for step in steps {
            assert!(
                !closed,
                "overlay {n} closed before {step:?}:\n{}",
                self.all_frames()
            );
            keys.clear();
            match step {
                Step::Key(k) => keys.push(k),
                Step::Type(text) => keys.extend(
                    text.chars()
                        .map(|c| OverlayKey::plain(OverlayKeyCode::Char(c))),
                ),
                Step::WaitFor(text) => {
                    let deadline = std::time::Instant::now() + Duration::from_secs(10);
                    while !self.paint(n, overlay).contains(text) {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "overlay {n} never showed {text:?}:\n{}",
                            self.all_frames()
                        );
                        tick(overlay);
                    }
                }
                Step::UntilClosed => {
                    let deadline = std::time::Instant::now() + Duration::from_secs(10);
                    while !overlay.should_close() {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "overlay {n} never closed:\n{}",
                            self.all_frames()
                        );
                        self.paint(n, overlay);
                        tick(overlay);
                    }
                    closed = true;
                }
            }
            for k in keys.drain(..) {
                self.paint(n, overlay);
                if overlay.handle_key(k) == OverlayOutcome::Close {
                    closed = true;
                }
            }
        }
        closed = closed || overlay.should_close();
        assert!(closed, "overlay {n} left open:\n{}", self.all_frames());
    }
}

impl HostServices for OverlayHost {
    fn session_id(&self) -> Option<String> {
        Some(MY_SESSION_ID.to_string())
    }
    fn session_name(&self) -> Option<String> {
        Some("alice".to_string())
    }
    fn session_file(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/h/.cyrup/sessions/alice.jsonl"))
    }
    fn current_model(&self) -> Option<String> {
        Some("faux/faux-1".to_string())
    }
    fn session_context_messages(&self) -> BoxFuture<'_, Vec<cyrup_session::AgentMessage>> {
        Box::pin(async {
            vec![
                cyrup_session::AgentMessage::core(LlmMessage::User {
                    content: vec![Content::text("please port the schema fix")],
                    timestamp: 0,
                }),
                cyrup_session::AgentMessage::core(LlmMessage::Assistant(faux_assistant_message(
                    vec![faux_text("done, tests pass")],
                    StopReason::Stop,
                ))),
            ]
        })
    }
    fn complete_standalone<'a>(
        &'a self,
        request: StandaloneCompletion,
        cancel: CancelToken,
    ) -> BoxFuture<'a, Result<AssistantMessage, StandaloneCompletionRefusal>> {
        self.requests.lock().unwrap().push(request);
        match self.completion {
            Completion::Reply(text) => Box::pin(async move {
                Ok(faux_assistant_message(
                    vec![faux_text(text)],
                    StopReason::Stop,
                ))
            }),
            Completion::HangUntilCancelled => {
                let seen = self.completion_cancelled.clone();
                Box::pin(async move {
                    cancel.cancelled().await;
                    *seen.lock().unwrap() = true;
                    Err(StandaloneCompletionRefusal::Cancelled)
                })
            }
        }
    }
    fn open_overlay(&self, mut overlay: Box<dyn InteractiveOverlay>) -> bool {
        let script = self.scripts.lock().unwrap().pop_front();
        let Some(Script::Drive(steps)) = script else {
            return false;
        };
        let n = self.overlays_opened.fetch_add(1, Ordering::SeqCst);
        // The live host blocks the calling task the same way (`block_in_place`).
        tokio::task::block_in_place(|| self.drive(n, overlay.as_mut(), steps));
        true
    }
    fn editor(&self, title: &str, initial: &str) -> Option<String> {
        self.editors
            .lock()
            .unwrap()
            .push((title.to_string(), initial.to_string()));
        (self.edit)(initial)
    }
    fn input(&self, prompt: &str, placeholder: Option<&str>, _: &DialogOptions) -> Option<String> {
        self.inputs
            .lock()
            .unwrap()
            .push((prompt.to_string(), placeholder.map(str::to_string)));
        self.input_answer.clone()
    }
    fn confirm(&self, _: &str, _: &str, _: &DialogOptions) -> bool {
        true
    }
    fn notify(&self, message: &str, kind: NotifyKind) {
        self.notified
            .lock()
            .unwrap()
            .push((message.to_string(), kind));
    }
    fn append_entry(&self, custom_type: &str, data: &serde_json::Value) -> Result<String, String> {
        self.entries
            .lock()
            .unwrap()
            .push((custom_type.to_string(), data.clone()));
        Ok("entry".to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// The rig: a real broker, this session through the production SessionStart, and peers
// ---------------------------------------------------------------------------------------------

struct Live {
    ext: Arc<IntercomExtension>,
    host: Arc<OverlayHost>,
    broker: tokio::process::Child,
    socket: PathBuf,
    peers: Vec<Arc<IntercomClient>>,
    _agent_dir: tempfile::TempDir,
}

impl Live {
    async fn start(host: OverlayHost, cwd: &Path) -> Self {
        let agent_dir = tempfile::tempdir().expect("tempdir");
        let intercom_dir = intercom_dir_path(agent_dir.path());
        write_broker_command(&intercom_dir);
        let socket = broker_socket_path(&intercom_dir);
        let broker = spawn_broker(agent_dir.path());
        wait_for_broker(&socket, Duration::from_secs(20))
            .await
            .expect("broker up");
        let host = Arc::new(host);
        let ext = Arc::new(
            IntercomExtension::new(
                agent_dir.path().to_path_buf(),
                cwd.to_path_buf(),
                load_config(&intercom_dir).expect("config loads"),
                None,
            )
            .expect("build the extension"),
        );
        ext.set_host_services(host.clone());
        let ctx = tui();
        let _ = ext
            .on_event(
                &HostEvent::SessionStart {
                    reason: "test".to_string(),
                    previous_session_file: None,
                },
                &ctx,
            )
            .await;
        let state = ext.state().clone();
        assert!(
            within(Duration::from_secs(30), || state
                .client()
                .is_some_and(|c| c.is_connected()))
            .await,
            "the session connects on SessionStart"
        );
        Self {
            ext,
            host,
            broker,
            socket,
            peers: Vec::new(),
            _agent_dir: agent_dir,
        }
    }

    /// A connected peer; `last_activity` orders the picker.
    async fn peer(
        &mut self,
        id: &str,
        name: &str,
        cwd: &str,
        last_activity: u64,
    ) -> (
        Arc<IntercomClient>,
        tokio::sync::broadcast::Receiver<InboundEvent>,
    ) {
        let mut reg = registration(name);
        reg.cwd = cwd.to_string();
        reg.last_activity = last_activity.into();
        let peer = Arc::new(
            IntercomClient::connect(&self.socket, reg, Some(id.to_string()))
                .await
                .expect("the peer registers"),
        );
        let events = peer.subscribe();
        self.peers.push(peer.clone());
        (peer, events)
    }

    async fn command(&self, name: &str, args: &str, ctx: &HostCtx) -> Option<String> {
        tokio::time::timeout(
            Duration::from_secs(60),
            self.ext.execute_command(name, args, ctx),
        )
        .await
        .expect("the command finishes")
        .expect("the command dispatches")
    }

    async fn stop(mut self) {
        for peer in &self.peers {
            peer.disconnect();
        }
        if let Some(c) = self.ext.state().client() {
            c.disconnect();
        }
        let _ = self.broker.kill().await;
    }
}

fn tui() -> HostCtx {
    HostCtx::command(ExtMode::Tui, true, PathBuf::from("/tmp"))
}

async fn next_message(rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let InboundEvent::Message { message, .. } = rx.recv().await.expect("delivers") {
                return message.content.text;
            }
        }
    })
    .await
    .expect("the peer receives a message")
}

async fn receives_nothing(rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(400);
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        assert!(
            !matches!(event, InboundEvent::Message { .. }),
            "nothing may reach the peer: {event:?}"
        );
    }
}

/// The loader shows, then closes itself once the summary is generated.
fn loader_runs() -> Script {
    Script::Drive(vec![
        Step::WaitFor("Generating handover..."),
        Step::UntilClosed,
    ])
}

// ---------------------------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------------------------

/// `/handover <target> <next task>`: the loader runs the session's model over the conversation
/// with the goal, the human edits the summary, and the PEER receives the EDITED text through the
/// shared delivery, which also writes the `intercom_sent` entry. The command reports
/// `Handover: <delivery text>` (`index.ts:3145-3146`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_handover_target_delivers_the_human_edited_summary() {
    let mut live = Live::start(
        OverlayHost::new(vec![loader_runs()]),
        Path::new("/tmp/work"),
    )
    .await;
    let (_peer, mut inbox) = live.peer(PEER_SESSION_ID, "reviewer", "/tmp/peer", 1).await;

    let out = live
        .command(HANDOVER_COMMAND, "reviewer  port the fix next", &tui())
        .await;
    assert_eq!(out.as_deref(), Some("Handover: Message sent to reviewer"));

    let received = next_message(&mut inbox).await;
    assert!(
        received.starts_with("# Handover from alice"),
        "the framed summary: {received}"
    );
    assert!(received.contains(SUMMARY), "{received}");
    assert!(
        received.ends_with("\n\nEdited by the human."),
        "the EDITED text is what leaves: {received}"
    );
    let requests = live.host.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .user_text
            .ends_with("## Goal for the receiving agent\n\nport the fix next"),
        "the rest of the line is the goal: {}",
        requests[0].user_text
    );
    let editors = live.host.editors.lock().unwrap().clone();
    assert_eq!(editors.len(), 1);
    assert_eq!(editors[0].0, "Edit handover");
    assert!(editors[0].1.contains(SUMMARY));
    let sent = live.host.sent_entries();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["to"], "reviewer");
    live.stop().await;
}

/// A blank editor result sends nothing and reports `Handover cancelled` (`index.ts:3133-3135`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blank_edit_cancels_the_handover() {
    let mut host = OverlayHost::new(vec![loader_runs()]);
    host.edit = Box::new(|_| Some("   \n".to_string()));
    let mut live = Live::start(host, Path::new("/tmp/work")).await;
    let (_peer, mut inbox) = live.peer(PEER_SESSION_ID, "reviewer", "/tmp/peer", 1).await;

    let out = live.command(HANDOVER_COMMAND, "reviewer", &tui()).await;
    assert_eq!(out.as_deref(), Some("Handover cancelled"));
    receives_nothing(&mut inbox).await;
    assert!(live.host.sent_entries().is_empty());
    live.stop().await;
}

/// Escape in the loader aborts the model call itself (the completion sees its token fire) and
/// nothing is sent; the editor never opens (`index.ts:3114,:3124-3126`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn escape_in_the_loader_aborts_generation_and_sends_nothing() {
    let mut host = OverlayHost::new(vec![Script::Drive(vec![
        Step::WaitFor("Generating handover..."),
        key(OverlayKeyCode::Escape),
    ])]);
    host.completion = Completion::HangUntilCancelled;
    let mut live = Live::start(host, Path::new("/tmp/work")).await;
    let (_peer, mut inbox) = live.peer(PEER_SESSION_ID, "reviewer", "/tmp/peer", 1).await;

    let out = live.command(HANDOVER_COMMAND, "reviewer", &tui()).await;
    assert_eq!(out.as_deref(), Some("Handover cancelled"));
    let cancelled = live.host.completion_cancelled.clone();
    assert!(
        within(Duration::from_secs(5), || *cancelled.lock().unwrap()).await,
        "Escape reached the model call"
    );
    assert!(live.host.editors.lock().unwrap().is_empty());
    receives_nothing(&mut inbox).await;
    live.stop().await;
}

/// Bare `/handover` opens the picker: local peers without this session or its subagent children,
/// most recently active first; Enter hands over to the highlighted one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bare_handover_opens_the_picker_and_enter_hands_over() {
    let host = OverlayHost::new(vec![
        Script::Drive(vec![
            Step::WaitFor("Hand over to"),
            key(OverlayKeyCode::Down),
            key(OverlayKeyCode::Enter),
        ]),
        loader_runs(),
    ]);
    let mut live = Live::start(host, Path::new("/tmp/work")).await;
    let (_older, mut older_inbox) = live.peer(PEER_SESSION_ID, "reviewer", "/tmp/peer", 1).await;
    let (_newer, mut newer_inbox) = live
        .peer("session-9999888877776666", "builder", "/tmp/build", 2)
        .await;
    let (_child, _) = live
        .peer(
            CHILD_SESSION_ID,
            "worker: You are the writer",
            "/tmp/child",
            3,
        )
        .await;

    let out = live.command(HANDOVER_COMMAND, "", &tui()).await;
    let picker = live.host.frames_of(0).join("\n");
    assert!(!picker.contains("You are the writer"), "{picker}");
    assert!(
        !picker.contains("alice ("),
        "this session is not listed: {picker}"
    );
    let (builder, reviewer) = (
        picker.find("builder (").expect("builder listed"),
        picker.find("reviewer (").expect("reviewer listed"),
    );
    assert!(builder < reviewer, "most recently active first:\n{picker}");
    assert!(
        picker.contains("Fetch sessions from other machines"),
        "{picker}"
    );
    // Down moved to the second row, `reviewer`, addressed by session id.
    assert_eq!(
        out.as_deref(),
        Some(format!("Handover: Message sent to {PEER_SESSION_ID}").as_str())
    );
    assert!(next_message(&mut older_inbox).await.contains(SUMMARY));
    receives_nothing(&mut newer_inbox).await;
    live.stop().await;
}

/// A path target is a PROJECT (`index.ts:3038-3039`): `./rel` and `~/…` resolve to the session
/// live in that directory. Both peers already exist, so no Herdr pane is launched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_path_target_hands_over_to_the_session_live_in_that_project() {
    let work = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(work.path().join("rel")).expect("mkdir");
    let home = std::env::home_dir().expect("a home directory");
    let home_project = home.join(format!(".cyrup-it-handover-{}", std::process::id()));
    let host = OverlayHost::new(vec![loader_runs(), loader_runs()]);
    let mut live = Live::start(host, work.path()).await;
    let rel_cwd = work.path().join("rel").to_string_lossy().into_owned();
    let (_a, mut rel_inbox) = live.peer(PEER_SESSION_ID, "rel-peer", &rel_cwd, 1).await;
    let (_b, mut home_inbox) = live
        .peer(
            "session-7777666655554444",
            "home-peer",
            &home_project.to_string_lossy(),
            1,
        )
        .await;

    let out = live
        .command(HANDOVER_COMMAND, "./rel ship it", &tui())
        .await;
    assert_eq!(out.as_deref(), Some("Handover: Message sent to rel-peer"));
    assert!(next_message(&mut rel_inbox).await.contains(SUMMARY));

    let leaf = home_project
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let out = live
        .command(HANDOVER_COMMAND, &format!("~/{leaf}"), &tui())
        .await;
    assert_eq!(out.as_deref(), Some("Handover: Message sent to home-peer"));
    assert!(next_message(&mut home_inbox).await.contains(SUMMARY));
    live.stop().await;
}

/// Outside the interactive terminal UI `/handover` refuses with upstream's sentence
/// (`index.ts:3026-3029`): as text without a UI, as an Error notification with one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_handover_outside_the_terminal_ui_points_at_the_tool() {
    let live = Live::start(OverlayHost::new(Vec::new()), Path::new("/tmp/work")).await;
    let headless = HostCtx::command(ExtMode::Print, false, PathBuf::from("/tmp"));
    assert_eq!(
        live.command(HANDOVER_COMMAND, "reviewer", &headless)
            .await
            .as_deref(),
        Some(HANDOVER_REQUIRES_TUI)
    );
    let rpc = HostCtx::command(ExtMode::Rpc, true, PathBuf::from("/tmp"));
    assert_eq!(live.command(HANDOVER_COMMAND, "", &rpc).await, None);
    assert_eq!(
        live.host.notified(),
        vec![(HANDOVER_REQUIRES_TUI.to_string(), NotifyKind::Error)]
    );
    assert_eq!(live.host.overlays_opened.load(Ordering::SeqCst), 0);
    live.stop().await;
}

/// Bare `/intercom` with a terminal UI opens the LIVE list; `h` opens the handover picker
/// preselected on that session with the task field focused (`index.ts:3191-3194`,
/// `handover-picker.ts:89-93`), so typing goes straight to the next task.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h_in_the_live_session_list_opens_the_picker_on_that_session() {
    let host = OverlayHost::new(vec![
        Script::Drive(vec![
            Step::WaitFor("h: Hand over"),
            key(OverlayKeyCode::Down),
            key(OverlayKeyCode::Char('h')),
        ]),
        Script::Drive(vec![
            Step::WaitFor("tab: List"),
            Step::Type("review the diff"),
            key(OverlayKeyCode::Enter),
        ]),
        loader_runs(),
    ]);
    let mut live = Live::start(host, Path::new("/tmp/work")).await;
    let (_a, mut a_inbox) = live.peer(PEER_SESSION_ID, "reviewer", "/tmp/peer", 1).await;
    let (_b, mut b_inbox) = live
        .peer("session-9999888877776666", "builder", "/tmp/build", 2)
        .await;

    let out = live.command(INTERCOM_COMMAND, "", &tui()).await;
    let list = live.host.frames_of(0).join("\n");
    assert!(list.contains("Current Session"), "{list}");
    assert!(
        list.contains("enter: Message • h: Hand over • escape/ctrl+c: Close"),
        "{list}"
    );
    // The list is broker order; whichever peer was second is the one `h` picked.
    let rows: Vec<&str> = list.lines().collect();
    let first_frame_end = rows.iter().position(|l| l.starts_with('╰')).unwrap();
    let names: Vec<&str> = rows[..first_frame_end]
        .iter()
        .filter_map(|l| {
            ["reviewer", "builder"]
                .into_iter()
                .find(|n| l.contains(&format!("{n} (")))
        })
        .collect();
    let picked = names[1];
    let picker = live.host.frames_of(1).join("\n");
    assert!(
        picker.contains(&format!("→ {picked} (")),
        "preselected: {picker}"
    );
    assert!(
        picker.contains("Next task (optional): > review the diff"),
        "{picker}"
    );
    let requests = live.host.requests.lock().unwrap().clone();
    assert!(requests[0].user_text.ends_with("review the diff"));
    let (picked_inbox, other_inbox) = if picked == "reviewer" {
        (&mut a_inbox, &mut b_inbox)
    } else {
        (&mut b_inbox, &mut a_inbox)
    };
    assert!(next_message(picked_inbox).await.contains(SUMMARY));
    receives_nothing(other_inbox).await;
    assert!(out.is_some_and(|o| o.starts_with("Handover: Message sent to session-")));
    live.stop().await;
}

/// `alt+m` opens the same live list (`index.ts:3243-3246`); Enter opens the compose box, which
/// sends from inside the overlay and writes `intercom_sent`; the shortcut's Info outcome is a
/// notification because a shortcut returns nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alt_m_opens_the_live_list_and_enter_composes_and_sends() {
    let host = OverlayHost::new(vec![
        Script::Drive(vec![
            Step::WaitFor("Other Sessions"),
            key(OverlayKeyCode::Enter),
        ]),
        Script::Drive(vec![
            Step::WaitFor("Send to: reviewer"),
            Step::Type("hello from the box"),
            key(OverlayKeyCode::Enter),
            Step::UntilClosed,
        ]),
    ]);
    let mut live = Live::start(host, Path::new("/tmp/work")).await;
    let (_peer, mut inbox) = live.peer(PEER_SESSION_ID, "reviewer", "/tmp/peer", 1).await;

    tokio::time::timeout(
        Duration::from_secs(60),
        live.ext.execute_shortcut(INTERCOM_SHORTCUT, &tui()),
    )
    .await
    .expect("the shortcut finishes")
    .expect("the shortcut dispatches");
    assert_eq!(next_message(&mut inbox).await, "hello from the box");
    assert_eq!(
        live.host.notified(),
        vec![("Message sent to reviewer".to_string(), NotifyKind::Info)]
    );
    let sent = live.host.sent_entries();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["to"], "reviewer");
    assert_eq!(sent[0]["message"]["text"], "hello from the box");
    // An unknown chord is not this extension's.
    assert!(live.ext.execute_shortcut("alt+x", &tui()).await.is_err());
    live.stop().await;
}

/// With no interactive surface (`open_overlay` → `false`) bare `/intercom` falls back to the text
/// rendering of the same list — pi's `!ctx.hasUI` branch reached at the seam.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_interactive_surface_falls_back_to_the_text_list() {
    let mut live = Live::start(
        OverlayHost::new(vec![Script::NoSurface]),
        Path::new("/tmp/work"),
    )
    .await;
    let (_peer, _inbox) = live.peer(PEER_SESSION_ID, "reviewer", "/tmp/peer", 1).await;
    let out = live
        .command(INTERCOM_COMMAND, "", &tui())
        .await
        .expect("text output");
    assert!(out.contains("Current Session"), "{out}");
    assert!(out.contains("reviewer ("), "{out}");
    assert!(out.contains("h: Hand over"), "{out}");
    live.stop().await;
}

/// The relay runner: `herdr machine list`, `herdr --machine <label> agent list`, then `ssh`.
struct Relay {
    stdin: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl CommandRunner for Relay {
    async fn run(
        &self,
        command: &str,
        args: &[&str],
        stdin: Option<&str>,
        _timeout: Option<Duration>,
    ) -> std::io::Result<CommandResult> {
        if command == "ssh" {
            self.stdin
                .lock()
                .unwrap()
                .push(stdin.unwrap_or_default().to_string());
        }
        Ok(CommandResult {
            stdout: match (command, args.first().copied()) {
                ("ssh", _) => r#"{"ok":true,"delivered":true,"id":"m-9"}"#.to_string(),
                (_, Some("machine")) => {
                    r#"[{"label":"workstation","target":"user@ws","enabled":true},{"label":"off","target":"user@off","enabled":false}]"#
                        .to_string()
                }
                _ => r#"{"agents":[{"agent":"cyrup","name":"remote-reviewer","cwd":"/r/proj","agent_status":"idle"}]}"#.to_string(),
            },
            stderr: String::new(),
            code: 0,
            timed_out: false,
        })
    }
}

/// "Fetch sessions from other machines" lists the remote agents through the session's runner;
/// picking one hands over as `name@machine` over the SSH relay, with the session-file line
/// dropped (`crossMachine: true`, `index.ts:3086-3088`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_picker_fetches_other_machines_and_hands_over_across_them() {
    let host = OverlayHost::new(vec![
        Script::Drive(vec![
            Step::WaitFor("Fetch sessions from other machines"),
            key(OverlayKeyCode::Up),
            key(OverlayKeyCode::Enter),
            Step::WaitFor("remote-reviewer@workstation · idle"),
            key(OverlayKeyCode::Down),
            key(OverlayKeyCode::Enter),
        ]),
        loader_runs(),
    ]);
    let live = Live::start(host, Path::new("/tmp/work")).await;
    let relay = Arc::new(Relay {
        stdin: Mutex::new(Vec::new()),
    });
    live.ext.state().set_cross_machine_runner(relay.clone());

    let out = live.command(HANDOVER_COMMAND, "", &tui()).await;
    assert_eq!(
        out.as_deref(),
        Some(
            "Handover: Message sent to remote-reviewer@workstation over SSH (origin identity is SSH-asserted)"
        )
    );
    let picker = live.host.frames_of(0).join("\n");
    assert!(
        !picker.contains("off"),
        "a disabled machine is not listed: {picker}"
    );
    assert!(picker.contains("/r/proj"), "{picker}");
    let stdin = relay.stdin.lock().unwrap().clone();
    assert_eq!(stdin.len(), 1);
    assert!(stdin[0].contains("Port the schema fix."), "{}", stdin[0]);
    assert!(
        !stdin[0].contains("alice.jsonl"),
        "a cross-machine handover names no local session file: {}",
        stdin[0]
    );
    live.stop().await;
}

/// The picker's project row asks for the path; a blank answer cancels (`index.ts:3089-3094`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_project_row_asks_for_a_path_and_a_blank_one_cancels() {
    let mut host = OverlayHost::new(vec![Script::Drive(vec![
        Step::WaitFor("+ New session in a project path…"),
        key(OverlayKeyCode::Down),
        key(OverlayKeyCode::Enter),
    ])]);
    host.input_answer = Some("  ".to_string());
    let mut live = Live::start(host, Path::new("/tmp/work")).await;
    let (_peer, mut inbox) = live.peer(PEER_SESSION_ID, "reviewer", "/tmp/peer", 1).await;

    let out = live.command(HANDOVER_COMMAND, "", &tui()).await;
    assert_eq!(out.as_deref(), Some("Handover cancelled"));
    assert_eq!(
        live.host.inputs.lock().unwrap().clone(),
        vec![(
            "Project path for the new session".to_string(),
            Some("~/dev/project".to_string())
        )]
    );
    assert!(
        live.host.requests.lock().unwrap().is_empty(),
        "nothing generated"
    );
    receives_nothing(&mut inbox).await;
    live.stop().await;
}
