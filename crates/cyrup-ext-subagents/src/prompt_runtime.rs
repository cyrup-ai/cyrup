//! Child-side subagent prompt runtime — ports pi `runs/shared/subagent-prompt-runtime.ts`.
//!
//! # Why this module exists at all
//!
//! [`crate::exec::structured`] ports pi's ENTIRE parent-side structured-output mechanism —
//! `create_structured_output_runtime`, `read_structured_output`,
//! `cleanup_structured_output_runtime`, `structured_output_instruction`, the two env-var
//! constants, and the [`crate::exec::structured::StructuredOutputRuntime`] struct. Every one of
//! them had ZERO callers outside their own file. The mechanism was ported faithfully and wired to
//! nothing.
//!
//! What ran instead was `exec::structured::extract_structured_output_value`, a heuristic that
//! scanned the child's assistant messages for the newest fenced ```json block. That had no pi
//! counterpart, and it quietly contradicted the very rule `structured.rs` documents: pi's defining
//! property (`structured-output.ts:157-159`) is that a missing capture file is a HARD failure "EVEN
//! WHEN prose was produced". A fenced block IS prose, so cyrup was accepting exactly what pi
//! rejects — while its own doc comment claimed otherwise. That heuristic is now DELETED (SUBA-S01's
//! residual pass); the capture file this module writes is the only channel there is.
//!
//! # The mechanism (pi `subagent-prompt-runtime.ts:279-313`)
//!
//! The parent writes the declared JSON Schema to a private file and passes two env vars to the
//! child: [`STRUCTURED_OUTPUT_SCHEMA_ENV`] (where to read the schema) and
//! [`STRUCTURED_OUTPUT_CAPTURE_ENV`] (where to write the value). Child-side, this runtime reads
//! both, builds `{ type: "object", properties: { value: <schema> }, required: ["value"] }` as the
//! tool's parameters — so the model is constrained by the caller's real schema, not a freeform
//! blob — validates on call, writes the capture file, and returns `terminate: true` to end the
//! step. The parent then reads that file back.
//!
//! Nesting the schema under `value` moves it one JSON-Pointer level deeper, so
//! [`create_structured_output_tool_parameters`] also repoints every wrapper-relative `$ref`
//! (`#/$defs/X` -> `#/properties/value/$defs/X`) before advertising it. Validation still runs
//! against the RAW schema, whose pointers resolve against the caller's own root.
//!
//! # Why a SEPARATE extension rather than a third `RegistrationMode`
//!
//! A plain (non-fanout) subagent child attaches no subagents extension at all —
//! `subagent_extension_for_env` returns `None` for it, matching pi (`extension/index.ts:243-245` registers
//! nothing). So the `structured_output` tool cannot come from that extension without perturbing a
//! gate that is deliberately closed.
//!
//! pi has the same split and solves it the same way: `runs/shared/pi-args.ts:13` points at
//! `subagent-prompt-runtime.ts` as its OWN extension, loaded into the child independently of the
//! orchestrator surface. This module is that extension.
//!
//! # The rest of the file (the child-side PROMPT runtime)
//!
//! `subagent-prompt-runtime.ts` is not only the structured-output tool. Its two other exports are
//! what make a child behave like a child at all, and both were unported until now:
//!
//! * **`before_agent_start` → [`rewrite_subagent_prompt`]** (`:97-113,323-341`). The parent writes
//!   the persona's `inheritProjectContext` / `inheritSkills` decision and the fanout grant into the
//!   child's env (`runs/shared/pi-args.ts:215-216,181` @v0.34.0; cyrup `exec/mod.rs`'s
//!   [`INHERIT_PROJECT_CONTEXT_ENV`]/[`INHERIT_SKILLS_ENV`] + `child_role_env`). NOTHING read them
//!   child-side, so `inheritProjectContext: false` was a pure no-op: the child re-assembled its own
//!   system prompt from its own cwd and happily inherited every `AGENTS.md`/`CLAUDE.md` the persona
//!   had asked to be spared. And no child was ever TOLD it was a child, so a delegated worker that
//!   inherited orchestration history would cheerfully keep orchestrating — launching its own
//!   subagents, re-running the parent's fanout — because nothing in its prompt said not to.
//! * **`context` → [`strip_parent_only_subagent_messages`]** (`:141-159,317-321`). A forked child
//!   starts from the PARENT's conversation, which is full of parent-only orchestration bookkeeping:
//!   `subagent-notify` completions, slash-command results, control notices, and the parent's own
//!   `subagent` tool calls/results. Left in place, the child reads its own history as evidence that
//!   it is the orchestrator.
//!
//! [CYRUP-DELTA] Two section-boundary adaptations, both forced by cyrup's own system-prompt shape:
//!
//! 1. pi's `stripProjectContext`/`stripInheritedSkills` scan for markdown headers
//!    (`"\n\n# Project Context\n\n…"`, `"\n\nThe following skills provide…"`) and cut to whichever
//!    NEXT header appears first — a heuristic forced by pi's header-only sectioning. cyrup's
//!    assembler (`cyrup-session/src/prompt/{builder,skills_inject}.rs`) emits both sections with
//!    explicit CLOSING tags (`</project_context>`, `</available_skills>`), so the port cuts on the
//!    real delimiters instead of guessing where a section ends. Same intent, exact boundaries.
//! 2. pi's `stripSubagentOrchestrationSkill` (`:83-87`, called UNCONDITIONALLY at `:108`) deletes
//!    the `pi-subagents` skill entry from an inherited prompt. It matches two shapes: pi's
//!    attribute form `<skill name="pi-subagents" …>…</skill>`, and the nested form whose body
//!    contains `<name>pi-subagents</name>`. cyrup's assembler emits ONLY the nested form
//!    (`cyrup-session/src/prompt/skills_inject.rs:34-46`), so [`strip_subagent_orchestration_skill`]
//!    ports the second replace and omits the first — there is no attribute form to match.
//!
//!    An earlier revision of this comment claimed the port was dead code because "this crate has no
//!    `skills/` directory and registers no skill". That was FALSE on both counts even when it was
//!    written: `crates/cyrup-ext-subagents/resources/skills/pi-subagents/SKILL.md` is a 58 KB file
//!    that has always shipped here, and it is now registered through the extension's
//!    `resources_discover` contribution (`extension.rs`), so a parent session's
//!    `<available_skills>` block genuinely carries a `pi-subagents` entry and a forked child
//!    genuinely inherits it. Stripping it is exactly what stops a delegated worker from reading its
//!    own prompt as a licence to orchestrate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_agent::AgentMessage;
use cyrup_core::tool::ExecMode;
use cyrup_core::{
    CancelToken, Content, ExtensionId, TerminateHint, Tool, ToolCallId, ToolError, ToolResult,
    ToolUpdateSink,
};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{EventKind, EventPatch, ExtError, HookOutcome, HostEvent};

use crate::exec::structured::{
    STRUCTURED_OUTPUT_CAPTURE_ENV, STRUCTURED_OUTPUT_INSTRUCTION, STRUCTURED_OUTPUT_SCHEMA_ENV,
    validate_structured_output,
};
use crate::spawn::nested_events::FANOUT_CHILD_ENV;

/// The extension id this child-side runtime registers under. Distinct from the orchestrator
/// extension's `subagents` id — the two never coexist in one process (a plain child gets only this
/// one; a root orchestrator gets only that one), but they are separate extensions, not two modes
/// of the same one.
pub const PROMPT_RUNTIME_EXTENSION_ID: &str = "subagent-prompt-runtime";

/// The tool name the child must call, and which
/// [`crate::exec::structured::STRUCTURED_OUTPUT_MISSING_ERROR`] names when it was never called.
pub const STRUCTURED_OUTPUT_TOOL_NAME: &str = "structured_output";

// =================================================================================================
// G90 — the CHILD half of `action: "steer"` (pi `registerSteeringInbox`,
// `runs/shared/subagent-prompt-runtime.ts:328-470` @v0.43.0)
// =================================================================================================

/// The env var carrying this child's own steer inbox directory — pi `SUBAGENT_STEER_INBOX_ENV`
/// (`runs/shared/pi-args.ts:32`, value `PI_SUBAGENT_STEER_INBOX`), written by the spawn plan from
/// [`crate::exec::RunOptions::steer_inbox_dir`].
///
/// Declared HERE, on the reader, and aliased by the writer (`exec/mod.rs`), matching the existing
/// convention this module already sets for `INHERIT_PROJECT_CONTEXT_ENV`/`INHERIT_SKILLS_ENV`: two
/// independently-spelled copies of a cross-process contract silently drifting apart is exactly the
/// write-only-flag defect the aliasing exists to prevent — and it is precisely the defect this
/// whole feature had, in its worse form: the parent wrote steer requests to disk and NO env var
/// existed at all, so nothing in the crate ever read them in production.
pub const STEER_INBOX_ENV: &str = "CYRUP_SUBAGENT_STEER_INBOX";

/// SUBA-049 — where this child publishes its steering capability once, at start. pi
/// `SUBAGENT_STEER_CAPABILITY_ENV` (`runs/shared/pi-args.ts:101`, value
/// `PI_SUBAGENT_STEER_CAPABILITY`), read at `subagent-prompt-runtime.ts:334`.
pub const STEER_CAPABILITY_ENV: &str = "CYRUP_SUBAGENT_STEER_CAPABILITY";

/// SUBA-049 — where this child writes one [`crate::background::control::SteerAck`] per consumed or
/// refused request. pi `SUBAGENT_STEER_ACK_DIR_ENV` (`runs/shared/pi-args.ts:102`, value
/// `PI_SUBAGENT_STEER_ACK_DIR`), read at `subagent-prompt-runtime.ts:335`.
///
/// Declared here, on the reader, for the same reason [`STEER_INBOX_ENV`] is — and with the same
/// history behind the convention: the request half of this channel spent an entire release as a
/// write-only file drop because the writer and the reader never shared a constant.
pub const STEER_ACK_DIR_ENV: &str = "CYRUP_SUBAGENT_STEER_ACK_DIR";

/// How often the child re-checks its inbox. pi `setInterval(flush, 250)`
/// (`subagent-prompt-runtime.ts:432`).
///
/// Upstream ALSO installs an `fs.watch` on the directory (`:232`) and treats the interval as the
/// portable safety net. cyrup keeps only the interval: this crate's own `notify`-based watcher is
/// built for run-level status files, the inbox is a directory of tiny files written at human
/// pace, and 250 ms is already upstream's own worst-case latency bound — a watcher would improve
/// the best case and add a second, racier code path to the one seam whose failure mode is
/// "guidance silently never arrives".
const STEER_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// pi `formatSteerMessage` (`subagent-prompt-runtime.ts:271-304` @v0.43.0) — the exact text the
/// child's model sees. Kept verbatim (including the trailing "do not restart" instruction, which is
/// what stops a steered child from throwing away the work it has already done).
#[must_use]
pub fn format_steer_message(request: &crate::background::control::SteerRequest) -> String {
    format!(
        "Mid-run steering from the parent orchestrator:\n\n{}\n\nIncorporate this guidance at the \
         next safe point. Do not restart the task unless the guidance explicitly asks you to.",
        request.message
    )
}

/// The child-side steering inbox: pi's `registerSteeringInbox` closure state, as a struct.
///
/// # Why this exists at all
///
/// `action: "steer"` was a DEAD LETTER. The parent validated a request, the detached runner routed
/// it into `<run_dir>/control/steer-targets/<i>/` — and nothing anywhere read that directory in
/// production, nor did any env var tell a child it existed. The tool's own success text conceded
/// as much ("Delivery requires a live Cyrup child session that supports mid-run steering"); there
/// was no such session, because nothing had been written to make one.
///
/// The capability to finish it was never missing. `cyrup_ext::host::HostServices::inject_message`
/// (`cyrup-ext/src/host/services.rs:311`) is the seam, live-implemented at
/// `cyrup-session-svc/src/host_services.rs:735` and routed to `AgentSession::send_user_message`,
/// which — see `session.rs:3671-3677` — STEERS the running turn while streaming and starts a fresh
/// prompt when idle. That is `pi.sendUserMessage(text, { deliverAs: "steer" })`, exactly. This very
/// crate already calls it from two other places (`background/watch.rs`'s completion sink and
/// `native_supervisor.rs`'s channel poller), and `NativeExtension::set_host_services`
/// (`cyrup-ext/src/native.rs:449`) is how a native extension is handed the backend.
///
/// # Lifecycle (pi `:199-258`)
///
/// * `session_start` → [`Self::start`]: create the directory, arm the 250 ms poller.
/// * any of `message_start`/`message_update`/`message_end`/`tool_execution_start`/
///   `tool_execution_end`/`turn_end` → [`Self::activate`]: start (idempotent), set `can_steer`,
///   flush now. Upstream's `canSteer` gate is what keeps guidance from being injected into a
///   session that has not begun a turn yet; cyrup adds the same gate for the same reason, plus one
///   of its own — [`Self::services`] is late-bound, so a flush before `set_host_services` has
///   nothing to inject into.
/// * `session_shutdown` → [`Self::dispose`]: stop the poller. A request still on disk stays on
///   disk, which is correct: it was never delivered.
pub struct SteeringInbox {
    /// `<run_dir>/control/steer-targets/<flatIndex>/` for THIS child.
    dir: PathBuf,
    /// SUBA-049 — `<run_dir>/control/steer-acks/<flatIndex>/` for THIS child, from
    /// [`STEER_ACK_DIR_ENV`]. `None` reproduces upstream's own `if (!ackDir … ) return;`
    /// (`subagent-prompt-runtime.ts:349`): the child still delivers, it just does not report.
    ack_dir: Option<PathBuf>,
    /// SUBA-049 — `<run_dir>/control/steer-capabilities/<flatIndex>.json`, from
    /// [`STEER_CAPABILITY_ENV`]. `None` degrades the same way.
    capability_path: Option<PathBuf>,
    /// SUBA-049 — this child's flat index, which every ack and the capability record carry so the
    /// parent can tell WHICH child of a fan-out answered. pi reads it from
    /// `SUBAGENT_CHILD_INDEX_ENV` (`subagent-prompt-runtime.ts:337`).
    child_index: usize,
    /// The late-bound capability backend (`NativeExtension::set_host_services`). `None` until the
    /// host binds it, and on a headless/default host it is bound to a backend whose
    /// `inject_message` denies — both of which degrade to "no steering", never to a panic.
    services: std::sync::Mutex<Option<Arc<dyn cyrup_ext::host::HostServices>>>,
    state: std::sync::Mutex<SteeringInboxState>,
}

/// SUBA-049 — one follow-up request parked until a turn boundary (pi's `queued` array entry,
/// `subagent-prompt-runtime.ts:339`).
#[derive(Clone, Debug)]
struct QueuedFollowUp {
    request: crate::background::control::SteerRequest,
    /// pi `ready` (`:339`): a follow-up queued DURING a turn is not eligible at that same turn's
    /// start — it becomes ready at the turn's END. Without this flag a mid-turn `follow_up` would
    /// be delivered by the very turn it was meant to follow.
    ready: bool,
}

/// Clears [`SteeringInboxState::flushing`] on every exit from [`SteeringInbox::flush`], including a
/// future-drop.
///
/// Upstream's `flush` is a SYNCHRONOUS `(): void` whose whole body sits in
/// `try { … } finally { flushing = false; }` (`subagent-prompt-runtime.ts:381-413`), so the latch
/// cannot outlive the call. cyrup's port is `async` and awaits at every `acknowledge`, at
/// `consume_steer_requests_from_dir` and at the write-back — and a Rust future can be dropped at any
/// one of them (the caller is an event handler; the runtime also drives `flush` from the poll task).
/// A plain fall-through assignment therefore reproduces the `try` and NOT the `finally`: one dropped
/// flush would latch `flushing = true` forever, every later flush would take the `:534` early
/// return, and the inbox would go permanently deaf — silently, since the requests keep being
/// consumed off disk but never acknowledged, so the parent's `await_steer_ack` only ever times out
/// to `pending`. This guard is that `finally`.
struct FlushGuard<'a> {
    state: &'a std::sync::Mutex<SteeringInboxState>,
}

impl Drop for FlushGuard<'_> {
    fn drop(&mut self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flushing = false;
    }
}

#[derive(Default)]
struct SteeringInboxState {
    /// pi `canSteer` (`:199`): set by the first turn-lifecycle event. Until then the session has no
    /// live turn to steer.
    can_steer: bool,
    /// pi `started` (`:202`): the poller is armed exactly once.
    started: bool,
    /// pi `disposed` (`:200`).
    disposed: bool,
    /// pi `flushing` (`:201`): re-entrancy guard. `consume_steer_requests_from_dir` DELETES each
    /// request as it reads it, so two overlapping flushes could not double-deliver — but they
    /// could interleave two orderings of one queue, and ordering is the one property the
    /// `(ts, id)` sort exists to guarantee.
    flushing: bool,
    /// SUBA-049 / pi `inTurn` (`:343`): whether an assistant turn is currently in flight. This is
    /// the whole of what `mode: "auto"` branches on.
    in_turn: bool,
    /// SUBA-049 / pi `queued` (`:339`): follow-ups parked for a turn boundary, capped at
    /// [`crate::background::control::MAX_STEER_QUEUE_SIZE`].
    queued: Vec<QueuedFollowUp>,
}

impl SteeringInbox {
    #[must_use]
    fn new(
        dir: PathBuf,
        ack_dir: Option<PathBuf>,
        capability_path: Option<PathBuf>,
        child_index: usize,
    ) -> Self {
        Self {
            dir,
            ack_dir,
            capability_path,
            child_index,
            services: std::sync::Mutex::new(None),
            state: std::sync::Mutex::new(SteeringInboxState::default()),
        }
    }

    /// SUBA-049 / pi's `acknowledge` closure (`subagent-prompt-runtime.ts:348-358`).
    ///
    /// A write failure is swallowed exactly as upstream's is: the acknowledgment is a diagnostic
    /// channel, and a child that cannot report must still deliver. Its absence is what the parent's
    /// acknowledgment timeout already covers.
    async fn acknowledge(
        &self,
        request: &crate::background::control::SteerRequest,
        state: crate::background::control::SteerAckState,
        message: &str,
        delivery_status: Option<crate::background::control::SteerDeliveryStatus>,
    ) {
        let Some(dir) = self.ack_dir.as_deref() else {
            return;
        };
        let ack = crate::background::control::SteerAck {
            kind: "steer-ack".to_string(),
            protocol_version: 1,
            request_id: request.id.clone(),
            index: self.child_index,
            state,
            message: message.to_string(),
            ts: crate::time::now_epoch_millis(),
            delivery_status,
        };
        let _ = crate::background::control::write_steer_ack_at(dir, &ack).await;
    }

    /// SUBA-049 / pi's `publishCapability` closure (`subagent-prompt-runtime.ts:360-363`).
    ///
    /// Republished on every `activate`, not only at `start`, and the reason is a Rust-side ordering
    /// fact rather than a preference: `NativeExtension::set_host_services` is LATE-BOUND, so at
    /// `session_start` this child genuinely does not yet know whether it can be steered. Upstream's
    /// `canSteer` is decidable at registration because `pi.sendUserMessage` either exists or does
    /// not. Publishing once here would therefore pin `supported: false` on every child that can in
    /// fact be steered — the exact inversion of what the record is for.
    ///
    /// [CYRUP-DELTA: republish cadence only. The record shape, the path and the semantics are pi's.]
    async fn publish_capability(&self) {
        let Some(path) = self.capability_path.as_deref() else {
            return;
        };
        let supported = self
            .services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        let capability = crate::background::control::SteerCapability {
            kind: "steer-capability".to_string(),
            protocol_version: 1,
            index: self.child_index,
            pid: std::process::id(),
            ready_at: crate::time::now_epoch_millis(),
            supported,
        };
        let _ = crate::background::control::write_steer_capability_at(path, &capability).await;
    }

    /// The inbox this child watches — exposed for tests and diagnostics.
    #[must_use]
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    fn bind_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        let mut slot = self
            .services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(services);
    }

    /// pi `start` (`:223-239`): create the directory, then arm the poller ONCE.
    ///
    /// A directory-creation failure returns without arming, exactly as upstream's `try/catch`
    /// around `mkdirSync` returns without setting `started` — a child that cannot see its inbox
    /// must not spin a poller against a path that will never exist.
    fn start(self: &Arc<Self>) {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.started || state.disposed {
                return;
            }
            if std::fs::create_dir_all(&self.dir).is_err() {
                return;
            }
            state.started = true;
        }
        let inbox = Arc::clone(self);
        tokio::spawn(async move {
            // SUBA-049 / pi `start`'s `publishCapability()` (`subagent-prompt-runtime.ts:227`):
            // announce this child BEFORE the first poll, so a parent that steers immediately finds
            // a capability record rather than an empty directory it cannot interpret. Done on the
            // poller task because `start` is sync (it is called from a sync `session_start` arm)
            // and the write is async.
            inbox.publish_capability().await;
            loop {
                tokio::time::sleep(STEER_POLL_INTERVAL).await;
                if inbox
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .disposed
                {
                    return;
                }
                inbox.flush().await;
            }
        });
    }

    /// pi `activate` (`:240-245`): start, allow steering, flush immediately. Called from every
    /// turn-lifecycle event so guidance lands at the first safe point rather than up to one poll
    /// interval later.
    async fn activate(self: &Arc<Self>) {
        self.start();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.disposed {
                return;
            }
            state.can_steer = true;
        }
        self.publish_capability().await;
        self.flush().await;
    }

    /// SUBA-049 / pi's `turn_start` handler (`subagent-prompt-runtime.ts:449-457`): a turn is now in
    /// flight, and exactly ONE ready follow-up is released into it.
    ///
    /// One, not all: upstream splices a single entry (`queued.splice(next, 1)`), so a burst of
    /// queued follow-ups is spread across turn boundaries rather than dumped into one turn. That is
    /// the difference between "follow-up" and "steer" surviving the queue.
    pub async fn on_turn_start(self: &Arc<Self>) {
        let released = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.disposed {
                return;
            }
            state.in_turn = true;
            state
                .queued
                .iter()
                .position(|entry| entry.ready)
                .map(|at| state.queued.remove(at))
        };
        if let Some(entry) = released {
            let delivered = self
                .services
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .is_some_and(|services| {
                    services
                        .inject_message(
                            &format_steer_message(&entry.request),
                            None,
                            true,
                            None,
                            false,
                        )
                        .is_ok()
                });
            if delivered {
                self.acknowledge(
                    &entry.request,
                    crate::background::control::SteerAckState::Delivered,
                    "Cyrup delivered the queued follow-up at a turn boundary.",
                    Some(crate::background::control::SteerDeliveryStatus::Delivered),
                )
                .await;
            } else {
                // Not upstream's branch, because upstream cannot reach it: `sendUserMessage` is
                // resolved once at registration and cannot start failing later. cyrup's host CAN
                // (the session may be tearing down), and a follow-up that was acknowledged `queued`
                // and then silently evaporated is precisely the fire-and-forget failure this item
                // exists to remove — so the terminal outcome is reported.
                self.acknowledge(
                    &entry.request,
                    crate::background::control::SteerAckState::Failed,
                    "Run ended before queued follow-up delivery.",
                    Some(crate::background::control::SteerDeliveryStatus::Queued),
                )
                .await;
            }
        }
        self.activate().await;
    }

    /// SUBA-049 / pi's `turn_end` handler (`subagent-prompt-runtime.ts:458-462`): the turn is over,
    /// so every parked follow-up becomes eligible for the NEXT one.
    pub async fn on_turn_end(self: &Arc<Self>) {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.disposed {
                return;
            }
            state.in_turn = false;
            for entry in &mut state.queued {
                entry.ready = true;
            }
        }
        self.activate().await;
    }

    /// pi `dispose` (`:252-258`) plus SUBA-049's `session_shutdown` acknowledgment sweep
    /// (`subagent-prompt-runtime.ts:465-469`): every follow-up still parked when the run ends is
    /// reported `failed`, because it was never delivered.
    ///
    /// This is the acknowledgment that makes the whole channel honest. Without it a `follow_up`
    /// steer against a child that finished before its next turn boundary would have been
    /// acknowledged `queued` and then silently vanished — a fire-and-forget outcome wearing a
    /// receipt.
    pub async fn dispose(&self) {
        let queued = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.disposed = true;
            state.can_steer = false;
            state.in_turn = false;
            std::mem::take(&mut state.queued)
        };
        for entry in queued {
            self.acknowledge(
                &entry.request,
                crate::background::control::SteerAckState::Failed,
                "Run ended before queued follow-up delivery.",
                Some(crate::background::control::SteerDeliveryStatus::Queued),
            )
            .await;
        }
    }

    /// pi `flush` (`:205-222`): drain the inbox in `(ts, id)` order and inject each request into
    /// the live session.
    ///
    /// On an injection failure the failed request is acknowledged `failed` and the requests AFTER
    /// it are written back to the inbox, then the drain stops (pi `:389-392`, whose write-back loop
    /// is over `requests.slice(index + 1)`). This is what makes the hand-off lossless across a
    /// transient host error:
    /// `consume_steer_requests_from_dir` removes each file as it reads it, so without the write-back
    /// a single failed inject would silently discard the rest of the queue.
    ///
    /// SUBA-049: every path out of this loop now writes exactly one
    /// [`crate::background::control::SteerAck`]. That is the invariant — a request that was
    /// consumed and not acknowledged is the fire-and-forget bug this item was filed for.
    async fn flush(&self) {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.disposed || state.flushing || !state.can_steer {
                return;
            }
            state.flushing = true;
        }
        // pi's `finally { flushing = false; }` (`:411-413`). Armed the instant the latch is set and
        // BEFORE the first `.await`, so no drop point between here and the end of the body can
        // strand it. See [`FlushGuard`].
        let _flush_guard = FlushGuard { state: &self.state };

        let services = self
            .services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        let requests = crate::background::control::consume_steer_requests_from_dir(&self.dir).await;
        for (index, request) in requests.iter().enumerate() {
            // SUBA-049 / pi `:370-372`. The `canSteer` half of upstream's guard is cyrup's
            // "`set_host_services` never bound a backend": both mean the child has no way to reach
            // its own session. Before this the request was simply dropped on the floor and the tool
            // still answered success.
            //
            // [CYRUP-DELTA: "Pi" -> "Cyrup" in the sentence, matching this crate's standing rebrand
            // of upstream's product noun in user-facing text — the same substitution
            // `STEER_FOREGROUND_RUN_REFUSAL` already carries.]
            let Some(services) = services.clone() else {
                self.acknowledge(
                    request,
                    crate::background::control::SteerAckState::Failed,
                    "Child Cyrup session does not support sendUserMessage steering.",
                    None,
                )
                .await;
                continue;
            };

            // pi `:374-375`: `follow_up` always parks; `auto` parks only when a turn is already in
            // flight; `steer` (and an absent mode) always interrupts.
            let requested_mode = request
                .mode
                .unwrap_or(crate::background::control::SteerDeliveryMode::Steer);
            let in_turn = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .in_turn;
            let park = matches!(
                requested_mode,
                crate::background::control::SteerDeliveryMode::FollowUp
            ) || (matches!(
                requested_mode,
                crate::background::control::SteerDeliveryMode::Auto
            ) && in_turn);

            if park {
                // pi `:376-380`: the cap is enforced BEFORE the message is accepted, and a request
                // over it is acknowledged `failed` with this exact sentence rather than silently
                // discarded. This is the item's own Verify.
                let accepted = {
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if state.queued.len() >= crate::background::control::MAX_STEER_QUEUE_SIZE {
                        false
                    } else {
                        // Queued DURING a turn -> not eligible until that turn ends.
                        let ready = !state.in_turn;
                        state.queued.push(QueuedFollowUp {
                            request: request.clone(),
                            ready,
                        });
                        true
                    }
                };
                if accepted {
                    self.acknowledge(
                        request,
                        crate::background::control::SteerAckState::Queued,
                        "Cyrup queued the correlated follow-up input.",
                        Some(crate::background::control::SteerDeliveryStatus::Queued),
                    )
                    .await;
                } else {
                    self.acknowledge(
                        request,
                        crate::background::control::SteerAckState::Failed,
                        &format!(
                            "Follow-up queue is full ({} messages).",
                            crate::background::control::MAX_STEER_QUEUE_SIZE
                        ),
                        None,
                    )
                    .await;
                }
                continue;
            }

            // `custom_type: None` is the load-bearing argument: it routes to
            // `AgentSession::send_user_message`, i.e. a real USER message the model must
            // answer, which is what `deliverAs: "steer"` means. A `Some(kind)` would make it a
            // custom (non-LLM) message the model never sees. `display: true` so the operator
            // watching the child's transcript sees the guidance arrive; `trigger_turn: true`
            // so an IDLE child (between turns) actually acts on it instead of parking it.
            match services.inject_message(&format_steer_message(request), None, true, None, true) {
                Ok(()) => {
                    // SUBA-049 / pi `:413`. Upstream can only report this once its own `input`
                    // event correlates the injected text back to the request; cyrup's
                    // `inject_message` reports acceptance synchronously at the call, so the
                    // acknowledgment is written here.
                    //
                    // [CYRUP-DELTA: no `input`-event round trip. Upstream needs the correlation
                    // because `pi.sendUserMessage` returns before the host has accepted; the
                    // `HostServices` seam does not, so the two-phase `pending` map has no work to
                    // do and would only introduce a window in which an accepted steer is
                    // unacknowledged.]
                    self.acknowledge(
                        request,
                        crate::background::control::SteerAckState::Delivered,
                        "Cyrup accepted the correlated steering input.",
                        Some(crate::background::control::SteerDeliveryStatus::Delivered),
                    )
                    .await;
                }
                Err(error) => {
                    // pi `:389-392`: report the failure with the host's own message, put the
                    // UNDELIVERED remainder back (including this one), and stop the drain.
                    self.acknowledge(
                        request,
                        crate::background::control::SteerAckState::Failed,
                        &error.to_string(),
                        None,
                    )
                    .await;
                    for pending in requests.get(index + 1..).unwrap_or_default() {
                        let _ = crate::background::control::write_steer_request_to_dir(
                            &self.dir, pending,
                        )
                        .await;
                    }
                    break;
                }
            }
        }

        // `flushing` is cleared by `_flush_guard`'s `Drop`, which also covers the drop paths a
        // trailing assignment here would miss.
    }
}

/// pi's exact tool description (`subagent-prompt-runtime.ts:299`).
const STRUCTURED_OUTPUT_TOOL_DESCRIPTION: &str =
    "Submit the required final structured output for this subagent step. This terminates the step.";

/// Child env flag: whether this subagent inherits the parent's project-context files
/// (`AGENTS.md`/`CLAUDE.md`) — pi `SUBAGENT_INHERIT_PROJECT_CONTEXT_ENV`
/// (`subagent-prompt-runtime.ts:29`), written parent-side by `exec/mod.rs`.
///
/// Declared HERE, next to the only code that reads it, and re-exported by the writer — a
/// write-only constant with no reader was exactly this item's defect.
pub const INHERIT_PROJECT_CONTEXT_ENV: &str = "CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT";

/// Child env flag: whether this subagent inherits the parent's skills — pi
/// `SUBAGENT_INHERIT_SKILLS_ENV` (`subagent-prompt-runtime.ts:30`).
///
/// The parent ALSO passes `--no-skills` when this is `0` (pi `runs/shared/pi-args.ts:155-157`, cyrup
/// `exec/mod.rs`), which stops the child DISCOVERING skills. This flag is the second half: a forked
/// child whose prompt already carries an inherited skills section still has to have it removed.
pub const INHERIT_SKILLS_ENV: &str = "CYRUP_SUBAGENT_INHERIT_SKILLS";

/// pi `CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS` (`subagent-prompt-runtime.ts:39-45`), verbatim.
pub const CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS: &str = concat!(
    "You are a child subagent, not the parent orchestrator.\n",
    "The parent session owns delegation, orchestration, review fanout, and follow-up worker launches.\n",
    "Ignore prior parent-only orchestration instructions in inherited conversation history.\n",
    "Do not propose or run subagents. Complete only your assigned role-specific task with the tools available to you.\n",
    "If you need to edit files, use the available editing tools. Do not print tool-call syntax, patches, or pseudo-tool calls as text.",
);

/// pi `CHILD_FANOUT_BOUNDARY_INSTRUCTIONS` (`subagent-prompt-runtime.ts:47-54`), verbatim. Used
/// instead of [`CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS`] for a child the parent DID authorize to fan
/// out ([`FANOUT_CHILD_ENV`] = `1`), so the grant is not contradicted by its own system prompt.
pub const CHILD_FANOUT_BOUNDARY_INSTRUCTIONS: &str = concat!(
    "You are a child subagent with explicit fanout responsibility for this assigned task.\n",
    "The parent session owns final orchestration, acceptance, and follow-up implementation launches.\n",
    "You may use the `subagent` tool only for the fanout work explicitly requested in this task.\n",
    "Do not broaden yourself into general parent orchestration. Do not launch follow-up workers unless the task explicitly asks for that.\n",
    "The maxSubagentDepth cap still applies and may block further fanout.\n",
    "If you need to edit files, use the available editing tools. Do not print tool-call syntax, patches, or pseudo-tool calls as text.",
);

/// pi `PARENT_ONLY_CUSTOM_MESSAGE_TYPES` (`subagent-prompt-runtime.ts:56-64`), verbatim. Every one
/// is orchestration bookkeeping the PARENT session produced about its children; a child that reads
/// them in its own history reads itself as the orchestrator.
///
/// Three of the seven are live cyrup producers today — `"subagent-notify"`
/// (`background/watch.rs`), `"subagent-slash-result"` (`registration/cost.rs`) and
/// `"subagent_control_notice"` (`tui/notices.rs`) — and the rest are kept because this is a POLICY
/// list, not an inventory: a forked child's inherited history can carry any customType a past
/// (or upstream-compatible) session wrote, and each of these is parent-only wherever it came from.
const PARENT_ONLY_CUSTOM_MESSAGE_TYPES: &[&str] = &[
    "subagent-orchestration-instructions",
    "subagent-slash-result",
    "subagent-slash-text-result",
    "subagent-notify",
    "subagent_control_notice",
    "subagent-control",
    "subagent-control-notice",
];

/// The orchestration tool a child must not read itself as having called
/// (`crate::extension::TOOL_NAME`; pi keys the same filters on the literal `"subagent"`,
/// `subagent-prompt-runtime.ts:124,129` @v0.34.0).
const SUBAGENT_TOOL_NAME: &str = "subagent";

/// Opening delimiter of cyrup's project-context section (`cyrup-session/src/prompt/builder.rs`'s
/// `project_context_open`). See the module doc's [CYRUP-DELTA] 1.
const PROJECT_CONTEXT_OPEN: &str = "<project_context>";
/// Closing delimiter of cyrup's project-context section (`builder.rs`'s `project_context_close`).
const PROJECT_CONTEXT_CLOSE: &str = "</project_context>";
/// First line of cyrup's skills section (`cyrup-session/src/prompt/skills_inject.rs`'s
/// `SKILLS_PREAMBLE`) — the section starts at the preamble, NOT at the `<available_skills>` tag.
const SKILLS_OPEN: &str = "Available skills (open the SKILL.md with the read tool to use one):";
/// Closing delimiter of cyrup's skills section (`skills_inject.rs`).
const SKILLS_CLOSE: &str = "</available_skills>";

/// What [`rewrite_subagent_prompt`] was told about this child (pi's three `readBooleanEnv` results
/// plus the structured-output presence check, `subagent-prompt-runtime.ts:111,330-338` @v0.34.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptRewriteOptions {
    /// `false` strips the inherited project-context section (pi `inheritProjectContext ?? true`).
    pub inherit_project_context: bool,
    /// `false` strips the inherited skills section (pi `inheritSkills ?? true`).
    pub inherit_skills: bool,
    /// `true` selects [`CHILD_FANOUT_BOUNDARY_INSTRUCTIONS`] over
    /// [`CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS`] (pi `fanoutChild === true`).
    pub fanout_child: bool,
    /// Whether the structured-output capture var is set, which appends
    /// [`STRUCTURED_OUTPUT_INSTRUCTION`] under the boundary block (pi `:111`).
    pub structured_output: bool,
}

impl Default for PromptRewriteOptions {
    /// pi's own defaults for a var that is present-but-unreadable: inherit everything, plain child.
    fn default() -> Self {
        Self {
            inherit_project_context: true,
            inherit_skills: true,
            fanout_child: false,
            structured_output: false,
        }
    }
}

/// pi `readBooleanEnv` (`subagent-prompt-runtime.ts:70-132`): an ABSENT var is `None` (the caller's
/// default applies); a present var is `false` only for the exact string `"0"`, true otherwise.
fn read_boolean_env(get: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<bool> {
    get(name).map(|value| value != "0")
}

/// Excise `open …  close` (inclusive of both delimiters, and of the blank line the assembler emits
/// before the section) from `prompt`. Returns the input unchanged when either delimiter is absent
/// or they appear out of order — a prompt that does not contain the section is already correct.
fn strip_delimited_section(prompt: &str, open: &str, close: &str) -> String {
    let Some(start) = prompt.find(open) else {
        return prompt.to_string();
    };
    let Some(head) = prompt.get(..start) else {
        return prompt.to_string();
    };
    let Some(rest) = prompt.get(start..) else {
        return prompt.to_string();
    };
    let Some(close_at) = rest.find(close) else {
        return prompt.to_string();
    };
    let end = start.saturating_add(close_at).saturating_add(close.len());
    let Some(tail) = prompt.get(end..) else {
        return prompt.to_string();
    };
    // The assembler separates sections with a blank line; leaving it behind would accumulate
    // stray whitespace exactly where pi's header-anchored slice leaves none.
    let head = head.trim_end_matches(['\n', '\r', ' ', '\t']);
    let mut out = String::with_capacity(head.len().saturating_add(tail.len()));
    out.push_str(head);
    out.push_str(tail);
    out
}

/// pi `stripProjectContext` (`subagent-prompt-runtime.ts:145-150`), on cyrup's delimiters.
#[must_use]
pub fn strip_project_context(prompt: &str) -> String {
    strip_delimited_section(prompt, PROJECT_CONTEXT_OPEN, PROJECT_CONTEXT_CLOSE)
}

/// pi `stripInheritedSkills` (`subagent-prompt-runtime.ts:152-157`), on cyrup's delimiters.
#[must_use]
pub fn strip_inherited_skills(prompt: &str) -> String {
    strip_delimited_section(prompt, SKILLS_OPEN, SKILLS_CLOSE)
}

/// The orchestration skill's name, as it appears inside a `<name>` element of cyrup's
/// `<available_skills>` block. Deliberately the SAME constant
/// [`crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL`] carries — one name, two enforcement
/// points (that module refuses to RESOLVE it for a child; this one removes it from a prompt the
/// child INHERITED), and they must never drift.
const SUBAGENT_ORCHESTRATION_SKILL: &str = crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL;

/// pi `stripSubagentOrchestrationSkill` (`subagent-prompt-runtime.ts:159-163`): remove the
/// `pi-subagents` entry from an inherited `<available_skills>` block, leaving every other skill in
/// place.
///
/// Ports upstream's SECOND replace — the nested `<skill>…<name>pi-subagents</name>…</skill>` form,
/// which is the only shape cyrup's assembler emits (`skills_inject.rs:34-46`). Upstream's first
/// replace targets an attribute form (`<skill name="pi-subagents">`) cyrup never produces.
///
/// Unlike [`strip_inherited_skills`], this runs for EVERY child — including one that inherits
/// skills — because the orchestration skill is parent-only regardless of the inherit flag (pi calls
/// it unconditionally at `:108`, outside both `if` guards).
#[must_use]
pub fn strip_subagent_orchestration_skill(prompt: &str) -> String {
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(open_at) = rest.find(SKILL_OPEN) {
        let Some(head) = rest.get(..open_at) else {
            break;
        };
        let Some(from_open) = rest.get(open_at..) else {
            break;
        };
        let Some(close_rel) = from_open.find(SKILL_CLOSE) else {
            // An unterminated `<skill>` is not a block; emit the remainder verbatim.
            break;
        };
        let end = close_rel.saturating_add(SKILL_CLOSE.len());
        let Some(block) = from_open.get(..end) else {
            break;
        };
        out.push_str(head);
        if !block_names_orchestration_skill(block) {
            out.push_str(block);
        } else {
            // Upstream's replacement is the empty string AND its pattern consumes the block's
            // trailing whitespace (`<\/skill>\s*`), so removing the entry leaves no blank line
            // where it used to be.
            rest = from_open.get(end..).unwrap_or("");
            let trimmed = rest.trim_start_matches([' ', '\t', '\r', '\n']);
            // Keep the indentation-free remainder, but never swallow the block's own closing
            // `</available_skills>` line separator entirely: re-emit a single newline so the
            // following element still starts on its own line.
            if !trimmed.is_empty() && rest.len() != trimmed.len() {
                out.push('\n');
            }
            rest = trimmed;
            continue;
        }
        rest = from_open.get(end..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Opening tag of one entry in cyrup's `<available_skills>` block (`skills_inject.rs:34`).
const SKILL_OPEN: &str = "<skill>";
/// Closing tag of one entry (`skills_inject.rs:46`).
const SKILL_CLOSE: &str = "</skill>";

/// pi `SUBAGENT_ORCHESTRATION_SKILL_NAME_PATTERN` (`:47`, `/<name>\s*pi-subagents\s*<\/name>/`):
/// does this `<skill>` block name the orchestration skill?
fn block_names_orchestration_skill(block: &str) -> bool {
    let mut rest = block;
    while let Some(at) = rest.find("<name>") {
        let Some(after) = rest.get(at.saturating_add("<name>".len())..) else {
            return false;
        };
        let Some(close) = after.find("</name>") else {
            return false;
        };
        if after.get(..close).unwrap_or("").trim() == SUBAGENT_ORCHESTRATION_SKILL {
            return true;
        }
        rest = after.get(close..).unwrap_or("");
    }
    false
}

/// pi `stripChildBoundaryInstructions` (`subagent-prompt-runtime.ts:165-171`): remove any boundary
/// block already present, then drop the leading blank lines that removal leaves.
///
/// Load-bearing for IDEMPOTENCE, not cosmetics: a child whose persona body was appended to a prompt
/// that already carried a boundary block (a fork, a resumed session, a re-entrant
/// `before_agent_start`) must end up with exactly ONE boundary block, and it must be the one this
/// run's flags select — not a stale `fanout` block from a run that was granted fanout.
fn strip_child_boundary_instructions(prompt: &str) -> String {
    let mut rewritten = prompt.to_string();
    for boundary in [
        CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS,
        CHILD_FANOUT_BOUNDARY_INSTRUCTIONS,
    ] {
        rewritten = rewritten.replace(boundary, "");
    }
    trim_leading_blank_lines(&rewritten).to_string()
}

/// pi's `.replace(/^(?:[ \t]*\r?\n)+/, "")` (`subagent-prompt-runtime.ts:170`): drop whole leading
/// BLANK lines only. A leading space on a non-blank first line is preserved, exactly as the regex
/// requires — the alternative (`trim_start`) would silently reflow an indented prompt body.
fn trim_leading_blank_lines(text: &str) -> &str {
    let mut rest = text;
    while let Some(line_end) = rest.find('\n') {
        let Some(first_line) = rest.get(..line_end) else {
            break;
        };
        if !first_line
            .chars()
            .all(|c| c == ' ' || c == '\t' || c == '\r')
        {
            break;
        }
        match rest.get(line_end.saturating_add(1)..) {
            Some(next) => rest = next,
            None => break,
        }
    }
    rest
}

/// pi `rewriteSubagentPrompt` (`subagent-prompt-runtime.ts:173-189`): strip what this child was told
/// not to inherit, remove any pre-existing boundary block, then PREFIX the boundary block this run
/// selects (plus the structured-output instruction when a schema was declared).
#[must_use]
pub fn rewrite_subagent_prompt(prompt: &str, opts: &PromptRewriteOptions) -> String {
    let mut rewritten = prompt.to_string();
    if !opts.inherit_project_context {
        rewritten = strip_project_context(&rewritten);
    }
    if !opts.inherit_skills {
        rewritten = strip_inherited_skills(&rewritten);
    }
    // pi `:108` — UNCONDITIONAL, outside both `if` guards above: even a child that inherits every
    // other skill must not inherit the parent's orchestration skill.
    rewritten = strip_subagent_orchestration_skill(&rewritten);
    rewritten = strip_child_boundary_instructions(&rewritten);
    let boundary = if opts.fanout_child {
        CHILD_FANOUT_BOUNDARY_INSTRUCTIONS
    } else {
        CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS
    };
    let structured = if opts.structured_output {
        format!("\n\n{STRUCTURED_OUTPUT_INSTRUCTION}")
    } else {
        String::new()
    };
    format!("{boundary}{structured}\n\n{rewritten}")
}

/// pi `isParentOnlySubagentMessage` (`:115-120`): a `custom` message whose type is parent-only.
fn is_parent_only_custom(message: &AgentMessage) -> bool {
    match message {
        AgentMessage::Custom { kind, .. } => {
            PARENT_ONLY_CUSTOM_MESSAGE_TYPES.contains(&kind.as_str())
        }
        _ => false,
    }
}

/// pi `stripParentOnlySubagentMessages` (`subagent-prompt-runtime.ts:249-269`): drop the parent's
/// orchestration bookkeeping from the context a CHILD sends to the model.
///
/// Returns `None` when nothing changed, so the caller can leave the in-flight list untouched
/// (pi's `if (messages === event.messages) return undefined;`, `:319`) rather than handing back an
/// identical copy the dispatcher would treat as a mutation.
///
/// `preserve_fanout_tool_history` is pi's `SUBAGENT_FANOUT_CHILD_ENV === "1"` check (`:142`): a
/// child that IS authorized to fan out keeps its own `subagent` calls and results, because for it
/// they are its own work rather than the parent's. A plain child keeps neither.
#[must_use]
pub fn strip_parent_only_subagent_messages(
    messages: &[Arc<AgentMessage>],
    preserve_fanout_tool_history: bool,
) -> Option<Vec<Arc<AgentMessage>>> {
    let mut changed = false;
    let mut filtered: Vec<Arc<AgentMessage>> = Vec::with_capacity(messages.len());
    for message in messages {
        let drop_subagent_tool_result = !preserve_fanout_tool_history
            && matches!(message.as_ref(), AgentMessage::ToolResult(tr) if tr.tool_name == SUBAGENT_TOOL_NAME);
        if is_parent_only_custom(message) || drop_subagent_tool_result {
            changed = true;
            continue;
        }
        if preserve_fanout_tool_history {
            filtered.push(Arc::clone(message));
            continue;
        }
        match strip_assistant_subagent_tool_calls(message) {
            // pi returns `undefined` for an assistant message left with NO content at all — the
            // message existed only to make the call, so it is dropped rather than sent empty.
            None => changed = true,
            Some(stripped) => {
                if !Arc::ptr_eq(&stripped, message) {
                    changed = true;
                }
                filtered.push(stripped);
            }
        }
    }
    changed.then_some(filtered)
}

/// pi `stripAssistantSubagentToolCallBlocks` (`:132-139`): remove `subagent` tool-call blocks from
/// an assistant message; `None` means the message became empty and must be dropped entirely.
/// Any non-assistant message passes through untouched.
/// Returns the SAME handle when nothing was stripped, so the caller's `Arc::ptr_eq` check is an
/// exact "was this message rewritten?" test and an untouched message is never re-allocated.
fn strip_assistant_subagent_tool_calls(message: &Arc<AgentMessage>) -> Option<Arc<AgentMessage>> {
    let AgentMessage::Assistant(assistant) = message.as_ref() else {
        return Some(Arc::clone(message));
    };
    let kept: Vec<Content> = assistant
        .content
        .iter()
        .filter(|block| !matches!(block, Content::ToolCall(tc) if tc.name == SUBAGENT_TOOL_NAME))
        .cloned()
        .collect();
    if kept.len() == assistant.content.len() {
        return Some(Arc::clone(message));
    }
    if kept.is_empty() {
        return None;
    }
    let mut assistant = (**assistant).clone();
    assistant.content = kept;
    Some(Arc::new(AgentMessage::Assistant(Arc::new(assistant))))
}

/// The JSON Pointer the caller's whole schema is relocated to once nested under the wrapper's
/// `value` property (pi `createStructuredOutputToolParameters`, `structured-output.ts:65`).
const STRUCTURED_OUTPUT_VALUE_POINTER: &str = "#/properties/value";

/// Keywords whose value is a MAP of name → subschema (pi `SCHEMA_MAP_KEYWORDS`,
/// `structured-output.ts:19`).
const SCHEMA_MAP_KEYWORDS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];

/// Keywords whose value is a SINGLE subschema (pi `SCHEMA_SINGLE_KEYWORDS`, `:20`).
const SCHEMA_SINGLE_KEYWORDS: &[&str] = &[
    "additionalItems",
    "additionalProperties",
    "contains",
    "not",
    "propertyNames",
    "if",
    "then",
    "else",
    "unevaluatedItems",
    "unevaluatedProperties",
    "contentSchema",
];

/// Keywords whose value is an ARRAY of subschemas (pi `SCHEMA_ARRAY_KEYWORDS`, `:21`).
const SCHEMA_ARRAY_KEYWORDS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

/// pi `rewriteLocalJsonPointerRefs` (`structured-output.ts:23-60`).
///
/// Nesting the caller's `outputSchema` under the tool parameters' `value` property MOVES every
/// node in it one resource-relative level deeper, which silently invalidates every local JSON
/// Pointer inside it: a schema that says `{"$defs":{"Node":…},"$ref":"#/$defs/Node"}` becomes a
/// tool schema whose `#/$defs/Node` resolves against the WRAPPER — where no `$defs` exists. The
/// model is then handed a schema it cannot satisfy (and a strict validator rejects the whole tool
/// definition). Rewriting `#` → `#/properties/value` and `#/x` → `#/properties/value/x` restores
/// every pointer.
///
/// `inherits_wrapper_resource` implements pi's `sharesWrapperResource` guard: a subschema that
/// declares its own `$id` starts a NEW JSON Schema resource, so its `#`-relative pointers resolve
/// against itself, not the wrapper — neither it nor anything beneath it is rewritten.
fn rewrite_local_json_pointer_refs(
    schema: &serde_json::Value,
    pointer_prefix: &str,
    inherits_wrapper_resource: bool,
) -> serde_json::Value {
    // pi `:24`: booleans, `null`, scalars and arrays pass through untouched.
    let serde_json::Value::Object(source) = schema else {
        return schema.clone();
    };
    let mut rewritten = source.clone();
    let shares_wrapper_resource =
        inherits_wrapper_resource && !source.get("$id").is_some_and(serde_json::Value::is_string);

    if shares_wrapper_resource {
        for keyword in ["$ref", "$dynamicRef", "$recursiveRef"] {
            let Some(serde_json::Value::String(reference)) = source.get(keyword) else {
                continue;
            };
            if reference == "#" {
                rewritten.insert(keyword.to_string(), pointer_prefix.into());
            } else if let Some(rest) = reference.strip_prefix('#')
                && rest.starts_with('/')
            {
                rewritten.insert(
                    keyword.to_string(),
                    format!("{pointer_prefix}{rest}").into(),
                );
            }
        }
    }

    let recurse = |nested: &serde_json::Value| {
        rewrite_local_json_pointer_refs(nested, pointer_prefix, shares_wrapper_resource)
    };

    for keyword in SCHEMA_MAP_KEYWORDS {
        let Some(serde_json::Value::Object(entries)) = source.get(*keyword) else {
            continue;
        };
        let mapped: serde_json::Map<String, serde_json::Value> = entries
            .iter()
            .map(|(name, nested)| (name.clone(), recurse(nested)))
            .collect();
        rewritten.insert((*keyword).to_string(), serde_json::Value::Object(mapped));
    }

    // `items` is either a tuple (draft-07 array form) or a single subschema (pi `:43-45`).
    match source.get("items") {
        Some(serde_json::Value::Array(items)) => {
            rewritten.insert(
                "items".to_string(),
                serde_json::Value::Array(items.iter().map(&recurse).collect()),
            );
        }
        Some(single) => {
            rewritten.insert("items".to_string(), recurse(single));
        }
        None => {}
    }

    for keyword in SCHEMA_SINGLE_KEYWORDS {
        if let Some(nested) = source.get(*keyword) {
            rewritten.insert((*keyword).to_string(), recurse(nested));
        }
    }

    for keyword in SCHEMA_ARRAY_KEYWORDS {
        if let Some(serde_json::Value::Array(items)) = source.get(*keyword) {
            rewritten.insert(
                (*keyword).to_string(),
                serde_json::Value::Array(items.iter().map(&recurse).collect()),
            );
        }
    }

    // Draft-07 `dependencies` is a union: a property-name ARRAY is data, not a schema (pi `:52-58`).
    if let Some(serde_json::Value::Object(dependencies)) = source.get("dependencies") {
        let mapped: serde_json::Map<String, serde_json::Value> = dependencies
            .iter()
            .map(|(name, nested)| {
                let value = if nested.is_array() {
                    nested.clone()
                } else {
                    recurse(nested)
                };
                (name.clone(), value)
            })
            .collect();
        rewritten.insert(
            "dependencies".to_string(),
            serde_json::Value::Object(mapped),
        );
    }

    serde_json::Value::Object(rewritten)
}

/// pi `createStructuredOutputToolParameters` (`structured-output.ts:62-69`): the caller's schema
/// nested under `value`, with every wrapper-relative JSON Pointer inside it rewritten.
#[must_use]
pub fn create_structured_output_tool_parameters(schema: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "value": rewrite_local_json_pointer_refs(schema, STRUCTURED_OUTPUT_VALUE_POINTER, true),
        },
        "required": ["value"],
        "additionalProperties": false,
    })
}

/// The child-side `structured_output` tool (pi `subagent-prompt-runtime.ts:288-313`).
pub struct StructuredOutputTool {
    /// The caller's declared JSON Schema, used to validate the submitted value.
    schema: serde_json::Value,
    /// `{ type: "object", properties: { value: <schema> }, required: ["value"],
    /// additionalProperties: false }` — pi builds the tool's parameters by NESTING the caller's
    /// schema under `value` rather than exposing it at the top level, so the model is constrained
    /// by the real schema instead of handed a freeform object. Built via
    /// [`create_structured_output_tool_parameters`], which also repoints the caller's local
    /// `$ref`s at their new depth; `schema` above stays the RAW schema, since validation of a
    /// submitted value resolves pointers against the caller's own root.
    parameters: serde_json::Value,
    /// Where the validated value is written for the parent to read back.
    output_path: PathBuf,
}

impl StructuredOutputTool {
    /// Build the tool for `schema`, capturing to `output_path`.
    #[must_use]
    pub fn new(schema: serde_json::Value, output_path: PathBuf) -> Self {
        let parameters = create_structured_output_tool_parameters(&schema);
        Self {
            schema,
            parameters,
            output_path,
        }
    }
}

#[async_trait]
impl Tool for StructuredOutputTool {
    fn name(&self) -> &str {
        STRUCTURED_OUTPUT_TOOL_NAME
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        STRUCTURED_OUTPUT_TOOL_DESCRIPTION
    }

    fn label(&self) -> Option<&str> {
        Some("Structured Output")
    }

    /// pi appends [`STRUCTURED_OUTPUT_INSTRUCTION`] to the CHILD's system prompt whenever the
    /// capture env var is set (`subagent-prompt-runtime.ts:111`). cyrup's extension API exposes no
    /// system-prompt append hook — `HostCtx::system_prompt` is read-only — but the `Tool` trait
    /// feeds exactly that section of the default system prompt via these two methods, so the
    /// instruction reaches the model by the idiomatic route instead of a bespoke one.
    ///
    /// This is also what finally makes [`crate::exec::structured::structured_output_instruction`] live: it was ported with
    /// pi's exact wording and then never called by anything.
    fn prompt_snippet(&self) -> Option<&str> {
        Some("structured_output: submit this step's required final structured result")
    }

    /// Per func-03 R-03-039 a guideline must NAME its tool so it stays meaningful once the tool is
    /// absent — pi's wording already does ("...calling the `structured_output` tool...").
    fn prompt_guidelines(&self) -> Vec<&str> {
        const GUIDELINES: &[&str] = &[STRUCTURED_OUTPUT_INSTRUCTION];
        GUIDELINES.to_vec()
    }

    /// Sequential, not [`ExecMode::Parallel`]: this call terminates the step and writes the single
    /// capture file the parent reads back, so it must not interleave with other tool calls.
    fn execution_mode(&self) -> ExecMode {
        ExecMode::Sequential
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let value = params.get("value").cloned().ok_or_else(|| {
            ToolError::new("structured_output requires a `value` conforming to the declared schema")
        })?;

        // pi throws here (`subagent-prompt-runtime.ts:303-305`), which surfaces to the model as a
        // tool error it can retry — the capture file is deliberately NOT written on an invalid
        // value, so the parent's read-back still reports "missing" rather than reading a value
        // that never passed validation.
        validate_structured_output(&self.schema, &value).map_err(|message| {
            ToolError::new(format!("Structured output validation failed: {message}"))
        })?;

        if let Some(dir) = self.output_path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| {
                ToolError::new(format!("Failed to write structured output: {err}"))
            })?;
        }
        let encoded = serde_json::to_vec(&value)
            .map_err(|err| ToolError::new(format!("Failed to encode structured output: {err}")))?;
        std::fs::write(&self.output_path, &encoded)
            .map_err(|err| ToolError::new(format!("Failed to write structured output: {err}")))?;

        // pi writes with `{ mode: 0o600 }`; the value can carry whatever the caller's schema
        // describes, so it gets the same owner-only treatment as the schema file itself.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&self.output_path, std::fs::Permissions::from_mode(0o600));
        }

        Ok(ToolResult {
            content: vec![Content::text("Structured output captured.")],
            details: Some(serde_json::json!({ "path": self.output_path.display().to_string() })),
            usage: None,
            added_tool_names: Vec::new(),
            terminate: TerminateHint::Terminate,
        })
    }
}

/// The child-side runtime extension: the optional `structured_output` tool, the
/// `before_agent_start` prompt rewrite, and the `context` history filter.
pub struct SubagentPromptRuntime {
    id: ExtensionId,
    /// `Some` only when this step declared an `outputSchema` (both structured env vars resolved).
    tool: Option<Arc<StructuredOutputTool>>,
    /// The NATIVE file-channel `contact_supervisor` — pi's `registerNativeSupervisorClient(pi)`
    /// (`subagent-prompt-runtime.ts:240`, added in `3ac0ef5`). `Some` only when this child has a
    /// resolvable supervisor channel AND `cyrup-intercom` will not be supplying its own
    /// broker-backed `contact_supervisor`
    /// ([`crate::native_supervisor::native_child_client_should_register`]) — cyrup's stand-in for
    /// upstream's `!hasTool(pi, "contact_supervisor")` guard, which `InitApi` cannot express.
    supervisor_tool: Option<Arc<crate::native_supervisor::NativeContactSupervisorTool>>,
    /// G106's SECOND child tool — the bare-named `intercom` fallback
    /// (`native-supervisor-channel.ts:305-321`). `Some` only when `contact_supervisor` also
    /// registers AND this agent's declared tool allowlist asked for `intercom`.
    intercom_fallback: Option<Arc<crate::native_supervisor::NativeChildIntercomTool>>,
    /// `None` reproduces pi's early return at `subagent-prompt-runtime.ts:333` — when NONE of the
    /// three child flags is defined the prompt is left exactly as assembled. In practice a real
    /// spawn always defines all three (`exec/mod.rs` writes both inherit flags and `child_role_env`
    /// writes the fanout flag), so this is the "not actually a subagent child" case.
    rewrite: Option<PromptRewriteOptions>,
    /// pi's `preserveCurrentFanoutToolHistory` (`:142`) — see
    /// [`strip_parent_only_subagent_messages`].
    preserve_fanout_tool_history: bool,
    /// pi's `registerToolBudget(pi, decodeToolBudgetEnv(process.env[TOOL_BUDGET_ENV]))`
    /// (`subagent-prompt-runtime.ts:263`). `Some` only when the parent shipped a budget in
    /// [`crate::exec::tool_budget::TOOL_BUDGET_ENV`]; `None` means every tool call passes
    /// untouched and this half of the runtime costs nothing.
    tool_budget: Option<ToolBudgetGuard>,
    /// G90 / pi `registerSteeringInbox` (`subagent-prompt-runtime.ts:328-470`). `Some` only when
    /// the parent handed this child a [`STEER_INBOX_ENV`] path — i.e. only for a background/async
    /// child, which is the only kind that has an async run directory to steer through.
    steering: Option<Arc<SteeringInbox>>,
    /// pi `registerChildWatchdog(pi)` (`subagent-prompt-runtime.ts:477`). `Some` only when the
    /// parent armed this child through
    /// [`crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV`] — the common case is `None`,
    /// since a child watchdog is off unless the orchestrator's own `subagents.watchdog.children`
    /// block turned it on. Owns its own [`crate::watchdog::runtime::MainWatchdogRuntime`], distinct
    /// from the orchestrator's ([`crate::extension::SubagentsExtension::watchdog`]).
    watchdog: Option<Arc<crate::watchdog::register_child::ChildWatchdog>>,
    /// The late-bound capability backend (`NativeExtension::set_host_services`), shared by the two
    /// halves of this runtime that need one at DELIVERY time rather than construction time: the
    /// child watchdog's warning sink, and SUBA-045's tool-availability diagnostic (which needs
    /// `all_tool_names()` — pi's `pi.getAllTools()` — at `agent_start`).
    ///
    /// `set_host_services` runs before `init`, but both consumers are built before BOTH (they have
    /// to be, to decide the subscription set), so each resolves through this slot when it fires.
    /// Shared with the closure handed to
    /// [`crate::watchdog::register_child::register_child_watchdog`].
    services: Arc<std::sync::Mutex<Option<Arc<dyn cyrup_ext::host::HostServices>>>>,
    /// pi `registerPermissionGate(pi)` (`subagent-prompt-runtime.ts:281-305,475`). `Some` only when
    /// the parent shipped a policy in
    /// [`crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV`] — upstream returns early on a
    /// missing policy (`:286`) and subscribes nothing.
    permission_gate: Option<PermissionGate>,
    /// SUBA-045 — pi's `refreshChildToolDiagnostic` inputs (`subagent-prompt-runtime.ts:98-103`).
    /// `Some` only when the parent wrote BOTH the diagnostic path and the required-tools list, which
    /// is the same gate upstream applies (`if (!filePath || !required) return undefined;`).
    tool_diagnostic: Option<ChildToolDiagnosticPlan>,
}

/// SUBA-045 — the child-side half of pi's `refreshChildToolDiagnostic`
/// (`subagent-prompt-runtime.ts:98-103`), resolved once from the environment at construction so the
/// `agent_start` handler is a registry read plus a diff.
#[derive(Clone, Debug)]
struct ChildToolDiagnosticPlan {
    /// `process.env[CHILD_TOOL_DIAGNOSTIC_PATH_ENV]`.
    path: PathBuf,
    /// `readRequiredChildTools()`.
    required: Vec<String>,
    /// `process.env[SUBAGENT_CHILD_AGENT_ENV]`, when the parent named the agent.
    agent: Option<String>,
    /// `readMcpDirectChildTools()`.
    mcp_direct_tools: Option<Vec<String>>,
}

/// pi `registerPermissionGate`'s closure state (`subagent-prompt-runtime.ts:281-305`): the decoded
/// policy, the audit path, the raw child-watchdog config the arbiter re-decodes for its model
/// selection, and the arbiter seam itself.
pub struct PermissionGate {
    policy: PermissionPolicy,
    raw_watchdog_config: Option<String>,
    audit_path: Option<PathBuf>,
    arbiter: Arc<dyn crate::watchdog::permission_arbiter::WatchdogPermissionAgent>,
}

impl std::fmt::Debug for PermissionGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionGate")
            .field("policy", &self.policy)
            .field("audit_path", &self.audit_path)
            .finish_non_exhaustive()
    }
}

/// What [`PERMISSION_POLICY_ENV`](crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV)
/// decoded to.
///
/// [CYRUP-DELTA] `Invalid` has no upstream shape: `decodePermissionRules` THROWS out of
/// `registerPermissionGate` and out of module init (`subagent-prompt-runtime.ts:285`), which kills
/// the child outright. That is fail-closed, and it must stay fail-closed here — treating an
/// undecodable policy as "no policy" would silently ungate every tool the parent meant to gate.
/// Blocking every tool call with the decode error is the same direction and strictly less severe
/// than killing the process, and it tells the operator what is wrong in the block reason.
#[derive(Debug, Clone)]
enum PermissionPolicy {
    Rules(crate::watchdog::permission_arbiter::PermissionRules),
    Invalid(String),
}

impl PermissionGate {
    /// pi's `tool_call` handler body (`subagent-prompt-runtime.ts:288-304`) — `Some(reason)` blocks
    /// the call, `None` lets it through.
    async fn evaluate(&self, tool_name: &str, input: &serde_json::Value) -> Option<String> {
        use crate::watchdog::permission_arbiter::{
            PermissionRuleDecision, WatchdogPermissionRequest, permission_decision,
            request_watchdog_permission,
        };
        let rules = match &self.policy {
            PermissionPolicy::Rules(rules) => rules,
            PermissionPolicy::Invalid(message) => {
                return Some(format!(
                    "Blocked by pi-subagents permission rule: the permission policy is invalid: \
                     {message}"
                ));
            }
        };
        match permission_decision(Some(rules), tool_name) {
            PermissionRuleDecision::Allow => None,
            PermissionRuleDecision::Deny => Some(format!(
                "Blocked by pi-subagents permission rule: '{tool_name}' is denied."
            )),
            PermissionRuleDecision::Ask => {
                let result = request_watchdog_permission(
                    &WatchdogPermissionRequest {
                        tool_name: tool_name.to_string(),
                        args: input.clone(),
                        raw_watchdog_config: self.raw_watchdog_config.clone(),
                        audit_path: self.audit_path.clone(),
                        cancel: None,
                    },
                    self.arbiter.as_ref(),
                )
                .await;
                (!result.approved)
                    .then(|| format!("Blocked by pi-subagents permission rule: {}", result.reason))
            }
        }
    }
}

/// The live counter behind a `toolBudget:` — pi's three `registerToolBudget` closure variables
/// (`toolCount`, `softNudged`, and the budget itself, `subagent-prompt-runtime.ts:306-325`).
///
/// `on_event` takes `&self`, so the mutable state lives behind a `Mutex` rather than in a JS
/// closure. A poisoned lock degrades to "do not block" — a budget is advisory scaffolding and must
/// never be the thing that kills a child run.
#[derive(Debug)]
struct ToolBudgetGuard {
    budget: crate::discovery::types::ResolvedToolBudget,
    state: std::sync::Mutex<ToolBudgetCounters>,
}

#[derive(Debug, Default)]
struct ToolBudgetCounters {
    tool_count: u32,
    soft_nudged: bool,
    /// The tool call whose RESULT should carry the one-time soft nudge — see
    /// [`SubagentPromptRuntime::on_event`]'s `[CYRUP-DELTA]` note on nudge delivery.
    pending_nudge: Option<(cyrup_core::ToolCallId, String)>,
}

impl SubagentPromptRuntime {
    /// The structured-output-only form (no prompt rewrite, no fanout grant).
    #[must_use]
    pub fn new(schema: serde_json::Value, output_path: PathBuf) -> Self {
        Self {
            id: ExtensionId::from(PROMPT_RUNTIME_EXTENSION_ID),
            tool: Some(Arc::new(StructuredOutputTool::new(schema, output_path))),
            supervisor_tool: None,
            intercom_fallback: None,
            rewrite: None,
            preserve_fanout_tool_history: false,
            tool_budget: None,
            steering: None,
            watchdog: None,
            services: Arc::new(std::sync::Mutex::new(None)),
            permission_gate: None,
            tool_diagnostic: None,
        }
    }

    /// Build from already-resolved parts. Kept env-free so callers (and tests) construct the exact
    /// runtime under test without touching process-global environment state.
    #[must_use]
    pub fn from_parts(
        tool: Option<Arc<StructuredOutputTool>>,
        rewrite: Option<PromptRewriteOptions>,
        preserve_fanout_tool_history: bool,
    ) -> Self {
        Self {
            id: ExtensionId::from(PROMPT_RUNTIME_EXTENSION_ID),
            tool,
            supervisor_tool: None,
            intercom_fallback: None,
            rewrite,
            preserve_fanout_tool_history,
            tool_budget: None,
            steering: None,
            watchdog: None,
            services: Arc::new(std::sync::Mutex::new(None)),
            permission_gate: None,
            tool_diagnostic: None,
        }
    }

    /// Attach the child-side steering inbox (pi `registerSteeringInbox`,
    /// `subagent-prompt-runtime.ts:328-585`). `None` leaves the child with no live steering channel,
    /// which is every foreground child.
    ///
    /// The request-only form. SUBA-049's return path (capability + acknowledgments) is opt-in
    /// through [`Self::with_steering_channel`], because upstream itself treats
    /// `SUBAGENT_STEER_ACK_DIR_ENV` / `SUBAGENT_STEER_CAPABILITY_ENV` as independently optional
    /// (`subagent-prompt-runtime.ts:334-335`, each guarded on its own): a child handed only an
    /// inbox still delivers, it just does not report.
    #[must_use]
    pub fn with_steering_inbox(self, dir: Option<PathBuf>) -> Self {
        self.with_steering_channel(dir, None, None, 0)
    }

    /// SUBA-049 — the full child-side steering channel: the request inbox, the acknowledgment
    /// directory, the capability file, and this child's flat index (which every record carries so a
    /// fan-out's parent can tell which child answered).
    #[must_use]
    pub fn with_steering_channel(
        mut self,
        dir: Option<PathBuf>,
        ack_dir: Option<PathBuf>,
        capability_path: Option<PathBuf>,
        child_index: usize,
    ) -> Self {
        self.steering = dir.map(|dir| {
            Arc::new(SteeringInbox::new(
                dir,
                ack_dir,
                capability_path,
                child_index,
            ))
        });
        self
    }

    /// The steering inbox this runtime watches, if any — exposed so a test can drive the real
    /// lifecycle instead of reaching into private state.
    #[must_use]
    pub fn steering_inbox(&self) -> Option<&Arc<SteeringInbox>> {
        self.steering.as_ref()
    }

    /// Attach the NATIVE file-channel `contact_supervisor` (pi `registerNativeSupervisorClient`,
    /// `subagent-prompt-runtime.ts:240`).
    #[must_use]
    pub fn with_supervisor_tool(
        mut self,
        tool: Option<Arc<crate::native_supervisor::NativeContactSupervisorTool>>,
    ) -> Self {
        self.supervisor_tool = tool;
        self
    }

    /// Attach G106's child-side `intercom` fallback (pi
    /// `registerNativeSupervisorClient(pi)` with `includeIntercomFallback` left on,
    /// `subagent-prompt-runtime.ts:275-277`).
    #[must_use]
    pub fn with_intercom_fallback(
        mut self,
        tool: Option<Arc<crate::native_supervisor::NativeChildIntercomTool>>,
    ) -> Self {
        self.intercom_fallback = tool;
        self
    }

    /// Attach the CHILD watchdog (pi `registerChildWatchdog(pi)`,
    /// `subagent-prompt-runtime.ts:477`). `None` — the common case — leaves the child unwatched.
    #[must_use]
    pub fn with_watchdog(
        mut self,
        watchdog: Option<Arc<crate::watchdog::register_child::ChildWatchdog>>,
        services: Arc<std::sync::Mutex<Option<Arc<dyn cyrup_ext::host::HostServices>>>>,
    ) -> Self {
        self.watchdog = watchdog;
        self.services = services;
        self
    }

    /// SUBA-045 — arm the tool-availability diagnostic from the parent's two env vars (pi
    /// `refreshChildToolDiagnostic`'s `filePath`/`required` pair, `subagent-prompt-runtime.ts:99-101`).
    ///
    /// Both must be present, which is upstream's `if (!filePath || !required) return undefined;`:
    /// the parent writes them together (`pi-args.ts:610-616`) or not at all, so one without the
    /// other is not a configuration to interpret.
    #[must_use]
    pub fn with_tool_diagnostic(mut self, get: &dyn Fn(&str) -> Option<String>) -> Self {
        use crate::exec::tool_availability as ta;
        let path = get(ta::CHILD_TOOL_DIAGNOSTIC_PATH_ENV)
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty());
        let required = crate::native_supervisor::read_required_child_tools(get);
        if let (Some(path), Some(required)) = (path, required) {
            self.tool_diagnostic = Some(ChildToolDiagnosticPlan {
                path: PathBuf::from(path),
                required,
                agent: get(crate::spawn::intercom_target::ENV_CHILD_AGENT)
                    .map(|raw| raw.trim().to_string())
                    .filter(|raw| !raw.is_empty()),
                mcp_direct_tools: ta::read_mcp_direct_child_tools(get),
            });
        }
        self
    }

    /// SUBA-045 — pi `refreshChildToolDiagnostic(pi)` (`subagent-prompt-runtime.ts:98-103`), fired
    /// from `agent_start` (`:514-516`).
    ///
    /// The available list is `pi.getAllTools().map((tool) => tool.name)`, which is
    /// [`cyrup_ext::host::HostServices::all_tool_names`]. A backend that cannot answer is treated as
    /// "no snapshot", and the refresh is SKIPPED rather than run against an empty registry — an
    /// empty answer would report every required tool as missing, which is the loudest possible way
    /// to be wrong. (Upstream cannot reach this state: `pi.getAllTools()` is synchronous and always
    /// present.)
    fn refresh_tool_diagnostic(&self) {
        let Some(plan) = &self.tool_diagnostic else {
            return;
        };
        let services = self
            .services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(available) = services.and_then(|s| s.all_tool_names()) else {
            return;
        };
        crate::exec::tool_availability::write_child_tool_diagnostic(
            &plan.path,
            &plan.required,
            &available,
            plan.agent.as_deref(),
            plan.mcp_direct_tools.as_deref(),
        );
    }

    /// The child watchdog this runtime drives, if any — exposed so a test can drive the real
    /// lifecycle instead of reaching into private state.
    #[must_use]
    pub fn watchdog(&self) -> Option<&Arc<crate::watchdog::register_child::ChildWatchdog>> {
        self.watchdog.as_ref()
    }

    /// Attach the child-side permission gate (pi `registerPermissionGate`,
    /// `subagent-prompt-runtime.ts:281-305`, called at `:475`).
    ///
    /// `raw_watchdog_config` is [`crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV`]'s raw
    /// value, forwarded verbatim because the arbiter re-decodes it for its own model selection
    /// (`permission-arbiter.ts:63,86`) — an `ask` in a child whose watchdog is off has no reviewer
    /// and therefore denies.
    #[must_use]
    pub fn with_permission_gate(
        mut self,
        encoded_policy: Option<&str>,
        raw_watchdog_config: Option<String>,
        audit_path: Option<PathBuf>,
        arbiter: Arc<dyn crate::watchdog::permission_arbiter::WatchdogPermissionAgent>,
    ) -> Self {
        // pi `:285-286`: `const rules = decodePermissionRules(...); if (!rules) return;` — no
        // policy means no handler at all.
        self.permission_gate =
            match crate::watchdog::permission_arbiter::decode_permission_rules(encoded_policy) {
                Ok(None) => None,
                Ok(Some(rules)) => Some(PermissionGate {
                    policy: PermissionPolicy::Rules(rules),
                    raw_watchdog_config,
                    audit_path,
                    arbiter,
                }),
                Err(message) => Some(PermissionGate {
                    policy: PermissionPolicy::Invalid(message),
                    raw_watchdog_config,
                    audit_path,
                    arbiter,
                }),
            };
        self
    }

    /// The permission gate this runtime enforces, if any — exposed so a test drives the real
    /// dispatch rather than reaching into private state.
    #[must_use]
    pub fn permission_gate(&self) -> Option<&PermissionGate> {
        self.permission_gate.as_ref()
    }

    /// Whether every half of this runtime is unarmed, i.e. registering it would install nothing.
    /// [`prompt_runtime_extension_from`] returns `None` in that case, which is upstream's "not
    /// actually a subagent child" state.
    #[must_use]
    pub fn is_inert(&self) -> bool {
        self.tool.is_none()
            && self.rewrite.is_none()
            && self.supervisor_tool.is_none()
            && self.intercom_fallback.is_none()
            && self.tool_budget.is_none()
            && self.steering.is_none()
            && self.watchdog.is_none()
            && self.permission_gate.is_none()
            // SUBA-045: a child armed with ONLY the tool-availability diagnostic is not inert — it
            // has a real `agent_start` job. Upstream never faces the question (it loads the runtime
            // into every child unconditionally); cyrup's `is_inert` gate is the cyrup-side stand-in
            // for that, so every half that does work has to be named here.
            && self.tool_diagnostic.is_none()
    }

    /// Attach the parent-supplied tool budget (pi `registerToolBudget`,
    /// `subagent-prompt-runtime.ts:306-325`). `None` leaves every tool call untouched.
    #[must_use]
    pub fn with_tool_budget(
        mut self,
        budget: Option<crate::discovery::types::ResolvedToolBudget>,
    ) -> Self {
        self.tool_budget = budget.map(|budget| ToolBudgetGuard {
            budget,
            state: std::sync::Mutex::new(ToolBudgetCounters::default()),
        });
        self
    }
}

#[async_trait]
impl NativeExtension for SubagentPromptRuntime {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// Ambient (SEAM-071/SEAM-074): the prompt runtime ships inside upstream's installed
    /// pi-subagents package, so it lives in the PATH tier `noExtensions` collapses
    /// (`resource-loader.ts:451-453` @v0.83.0) and is re-injected by path in a subagent child
    /// (`pi-subagents/src/runs/shared/pi-args.ts:413-417` @v0.47.1).
    fn is_ambient(&self) -> bool {
        true
    }

    /// Registers the tool when one exists and declares the two mutating seams pi's runtime hooks
    /// (`onRuntimeEvent("context", …)` `:317` and `onRuntimeEvent("before_agent_start", …)` `:323`).
    ///
    /// The subscription is not decoration: `Dispatcher::no_subscribers` short-circuits an event
    /// with no declared listener, so an unsubscribed handler is never called at all.
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        if let Some(tool) = &self.tool {
            api.register_tool(tool.clone());
        }
        // pi `registerSubagentPromptRuntime` calls `registerNativeSupervisorClient(pi)`
        // unconditionally (`subagent-prompt-runtime.ts:240`); the decision of whether a tool is
        // actually produced lives in the resolver below, exactly as upstream's own
        // `if (!readChildMetadata()) return;` early return does.
        if let Some(tool) = &self.supervisor_tool {
            api.register_tool(tool.clone());
        }
        // The fallback is registered AFTER `contact_supervisor`, matching upstream's own ordering
        // (`registerNativeSupervisorFallbackOnce` calls `registerNativeSupervisorClientOnce`
        // first): the primary tool must exist before a tool whose description tells the model to
        // "prefer contact_supervisor when available" is offered.
        if let Some(tool) = &self.intercom_fallback {
            api.register_tool(tool.clone());
        }
        // `context` is subscribed unconditionally, exactly as pi registers its handler
        // unconditionally: this extension exists ONLY inside a subagent child, and every subagent
        // child must have the parent's orchestration bookkeeping filtered out of its context.
        let mut kinds = vec![EventKind::Context];
        if self.rewrite.is_some() {
            kinds.push(EventKind::BeforeAgentStart);
        }
        // pi `registerToolBudget` subscribes `onRuntimeEvent("tool_call", …)` only when a budget
        // exists (`:172` returns early otherwise); `Dispatcher::no_subscribers` short-circuits an
        // event with no declared listener, so an un-budgeted child pays nothing for this.
        // `tool_result` is cyrup-only, and carries the soft nudge — see `on_event`.
        if self.tool_budget.is_some() {
            kinds.push(EventKind::ToolCall);
            kinds.push(EventKind::ToolResult);
        }
        // pi `registerPermissionGate` subscribes `onRuntimeEvent("tool_call", …)` only when a
        // policy decoded (`:285-287`). `EventKind` de-duplicates through the subscription bitset,
        // so a child with both a budget and a policy declares `tool_call` once.
        if self.permission_gate.is_some() {
            kinds.push(EventKind::ToolCall);
        }
        // G90 / pi `registerSteeringInbox`'s own `onRuntimeEvent` set
        // (`subagent-prompt-runtime.ts:441-464` @v0.43.0): `session_start` arms the poller,
        // `session_shutdown` disposes it, and the six turn-lifecycle events are the `activate`
        // triggers that set `canSteer` and flush immediately. Subscribed only when this child
        // actually has an inbox — `Dispatcher::no_subscribers` short-circuits the rest, so a
        // foreground child pays nothing for any of it (`message_update` in particular is
        // HIGH-FREQ).
        if self.steering.is_some() {
            kinds.extend_from_slice(&[
                EventKind::SessionStart,
                EventKind::SessionShutdown,
                // SUBA-049: `turn_start` joins the set, and it is not decoration — it is the only
                // event at which a parked `follow_up` can be released, so without it a
                // `mode:"follow_up"` steer would be acknowledged `queued` and never delivered.
                EventKind::TurnStart,
                EventKind::MessageStart,
                EventKind::MessageUpdate,
                EventKind::MessageEnd,
                EventKind::ToolExecStart,
                EventKind::ToolExecEnd,
                EventKind::TurnEnd,
            ]);
        }
        // pi `registerChildWatchdog`'s own five `onRuntimeEvent` registrations
        // (`watchdog/register-child.ts:89-115`). Subscribed only when the parent armed this child,
        // so an unwatched child pays nothing — `before_agent_start` is already in the set when a
        // prompt rewrite exists, and `EventKind` de-duplicates through the subscription bitset.
        if self.watchdog.is_some() {
            kinds.extend_from_slice(&[
                EventKind::SessionStart,
                EventKind::BeforeAgentStart,
                EventKind::TurnEnd,
                EventKind::AgentEnd,
                EventKind::SessionShutdown,
            ]);
        }
        // SUBA-045 / pi `onRuntimeEvent("agent_start", () => { refreshChildToolDiagnostic(pi); })`
        // (`subagent-prompt-runtime.ts:514-516`). Subscribed only when the parent armed the
        // diagnostic, so an agent with no explicit `tools:` allowlist declares no listener at all
        // and `Dispatcher::no_subscribers` short-circuits the event.
        if self.tool_diagnostic.is_some() {
            kinds.push(EventKind::AgentStart);
        }
        api.subscribe(&kinds);
        Ok(())
    }

    /// G90: the late-bound capability backend, forwarded to the steering inbox — this is the whole
    /// mechanism by which a steered message reaches the child's model
    /// (`HostServices::inject_message` → `AgentSession::send_user_message` → `steer` while
    /// streaming). Without it the inbox drains into nothing.
    fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        if let Some(steering) = &self.steering {
            steering.bind_services(Arc::clone(&services));
        }
        // The child watchdog's warning sink resolves through this slot — without it a displayed
        // child warning has nowhere to go (pi's sink is `pi.sendMessage`, always available).
        *self
            .services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(services);
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        // G90 / pi `registerSteeringInbox`'s handlers (`subagent-prompt-runtime.ts:441-469`). Run
        // FIRST and always fall through: every one of these events also has (or may later grow) a
        // meaning for the other halves of this runtime, and steering is a pure side effect that
        // must never swallow another handler's `HookOutcome`.
        if let Some(steering) = &self.steering {
            match ev {
                HostEvent::SessionStart { .. } => steering.start(),
                HostEvent::SessionShutdown { .. } => steering.dispose().await,
                // SUBA-049: `turn_start` and `turn_end` are no longer plain `activate` triggers —
                // they are the follow-up queue's clock (pi `:449-462`). `turn_start` releases ONE
                // ready follow-up into the turn that is beginning; `turn_end` makes the rest
                // eligible for the next one. Both still `activate` afterwards, exactly as upstream's
                // handlers `return activate()`.
                HostEvent::TurnStart { .. } => steering.on_turn_start().await,
                HostEvent::TurnEnd { .. } => steering.on_turn_end().await,
                HostEvent::MessageStart { .. }
                | HostEvent::MessageUpdate { .. }
                | HostEvent::MessageEnd { .. }
                | HostEvent::ToolExecStart { .. }
                | HostEvent::ToolExecEnd { .. } => steering.activate().await,
                _ => {}
            }
        }
        // SUBA-045 / pi `onRuntimeEvent("agent_start", …)` (`subagent-prompt-runtime.ts:514-516`).
        // Same rule as the two blocks around it: a pure side effect that falls through, never a
        // `HookOutcome`.
        if matches!(ev, HostEvent::AgentStart) {
            self.refresh_tool_diagnostic();
        }
        // pi `registerChildWatchdog`'s handlers (`watchdog/register-child.ts:89-115`). Like the
        // steering block above they run FIRST and always fall through: the watchdog observes, it
        // never blocks or mutates, so it must not swallow another handler's `HookOutcome`.
        if let Some(watchdog) = &self.watchdog {
            match ev {
                HostEvent::SessionStart { .. } => watchdog.handle_session_start(&_ctx.cwd),
                HostEvent::BeforeAgentStart {
                    prompt,
                    system_prompt,
                    ..
                } => watchdog.handle_before_agent_start(
                    &serde_json::json!({ "prompt": prompt, "systemPrompt": system_prompt }),
                    &_ctx.cwd,
                ),
                HostEvent::TurnEnd {
                    message,
                    tool_results,
                    ..
                } => watchdog.handle_turn_end(
                    &crate::watchdog::turn_delta::watchdog_turn_end_event(message, tool_results),
                    &_ctx.cwd,
                ),
                HostEvent::AgentEnd { .. } => watchdog.handle_agent_end(&_ctx.cwd).await,
                HostEvent::SessionShutdown { .. } => watchdog.handle_session_shutdown(),
                _ => {}
            }
        }
        // pi `registerPermissionGate`'s `tool_call` handler (`subagent-prompt-runtime.ts:288-304`).
        // It runs BEFORE the tool-budget handler below because upstream registers the two in that
        // order (`:475-476`) and pi's runner walks handlers in registration order
        // (`coding-agent/src/core/extensions/runner.ts:805-811`) — so a call the policy refuses is
        // never counted against the budget.
        if let Some(gate) = &self.permission_gate
            && let HostEvent::ToolCall { name, input, .. } = ev
            && let Some(reason) = gate.evaluate(name, input).await
        {
            return HookOutcome::Block {
                reason: Some(reason),
                // pi's `ToolCallEventResult.terminate` is `undefined` on every refusal it emits
                // (`extensions/types.ts:1072-1079` @v0.84.1); a permission refusal asks for THIS
                // call to be blocked, not for the batch to end.
                terminate: TerminateHint::Unspecified,
            };
        }
        match ev {
            // pi `:323-341`.
            HostEvent::BeforeAgentStart { system_prompt, .. } => {
                let Some(opts) = &self.rewrite else {
                    return HookOutcome::Noop;
                };
                let rewritten = rewrite_subagent_prompt(system_prompt, opts);
                // pi `:339`: an unchanged prompt returns nothing rather than a no-op mutation.
                if rewritten == *system_prompt {
                    HookOutcome::Noop
                } else {
                    HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                        system: Some(rewritten),
                        inject: None,
                    })
                }
            }
            // pi `registerToolBudget`'s `tool_call` handler (`:175-189`): count the call, fire the
            // one-time soft nudge, then BLOCK when the hard limit has been passed and this tool is
            // in the block set.
            HostEvent::ToolCall { call_id, name, .. } => {
                let Some(guard) = &self.tool_budget else {
                    return HookOutcome::Noop;
                };
                let Ok(mut state) = guard.state.lock() else {
                    // A poisoned lock must never be the thing that kills a child run; a budget is
                    // advisory scaffolding, so degrade to "allow".
                    return HookOutcome::Noop;
                };
                state.tool_count = state.tool_count.saturating_add(1);
                let tool_count = state.tool_count;
                if let Some(soft) = guard.budget.soft
                    && tool_count >= soft
                    && !state.soft_nudged
                {
                    state.soft_nudged = true;
                    // **[CYRUP-DELTA] — nudge transport only.** pi delivers the soft nudge via
                    // `pi.sendUserMessage(text, { deliverAs: "steer" })` (`:183`), a channel an
                    // event-tier `HostCtx` does not expose (`HostCtx` is data-only; there is no
                    // steer seam inside the loop). The nudge is therefore queued against THIS
                    // call's id and appended to its tool RESULT below — the same text, reaching
                    // the model at the same point in the transcript (immediately after the
                    // triggering call), in-band instead of out-of-band. pi's own comment calls the
                    // nudge advisory and the block authoritative; the block below is byte-identical
                    // to pi either way.
                    state.pending_nudge = Some((
                        call_id.clone(),
                        crate::exec::tool_budget::tool_budget_soft_nudge(&guard.budget, tool_count),
                    ));
                }
                if crate::exec::tool_budget::should_block_tool_for_budget(
                    &guard.budget,
                    name,
                    tool_count,
                ) {
                    return HookOutcome::Block {
                        reason: Some(crate::exec::tool_budget::tool_budget_blocked_message(
                            &guard.budget,
                            name,
                            tool_count,
                        )),
                        // A budget block exists so the child can FINALIZE with the tools left to
                        // it (`tool-budget.ts`'s `block` list), so it must not hint termination.
                        terminate: TerminateHint::Unspecified,
                    };
                }
                HookOutcome::Noop
            }
            // The soft-nudge delivery half of the `[CYRUP-DELTA]` above: append the queued nudge to
            // the result of the call that crossed the threshold, then clear it so it fires once.
            HostEvent::ToolResult {
                call_id, content, ..
            } => {
                let Some(guard) = &self.tool_budget else {
                    return HookOutcome::Noop;
                };
                let Ok(mut state) = guard.state.lock() else {
                    return HookOutcome::Noop;
                };
                let Some((pending_id, _)) = &state.pending_nudge else {
                    return HookOutcome::Noop;
                };
                if pending_id != call_id {
                    return HookOutcome::Noop;
                }
                let Some((_, nudge)) = state.pending_nudge.take() else {
                    return HookOutcome::Noop;
                };
                let mut content = content.clone();
                content.push(cyrup_core::Content::Text {
                    text: nudge.into(),
                    text_signature: None,
                });
                HookOutcome::Mutate(EventPatch::ToolResult {
                    content: Some(content),
                    details: None,
                    is_error: None,
                    usage: None,
                    terminate: None,
                })
            }
            // pi `:317-321`.
            HostEvent::Context { messages } => {
                match strip_parent_only_subagent_messages(
                    messages,
                    self.preserve_fanout_tool_history,
                ) {
                    Some(messages) => HookOutcome::Mutate(EventPatch::Context { messages }),
                    None => HookOutcome::Noop,
                }
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// Build the child-side runtime from this process's environment, or `None` when this process is not
/// a subagent child at all.
///
/// Two independent halves, matching pi — which loads `subagent-prompt-runtime.ts` into EVERY
/// subagent child (`runs/shared/pi-args.ts:141-143`) and then gates each half on its own vars:
///
/// * the `structured_output` tool needs BOTH structured vars (`:281`), plus a schema file that
///   reads and parses;
/// * the prompt rewrite needs at least ONE of the three child flags to be DEFINED (`:333`).
///
/// A process with neither gets `None` and carries no extra surface whatsoever. A malformed schema
/// is deliberately not a hard failure: the parent already validated it, so an unreadable file
/// child-side means the private temp dir is gone, and failing the child over it would turn a
/// recoverable "structured output missing" into an unexplained startup crash.
///
/// # Errors
/// CFG-080: a [`crate::exec::tool_budget::TOOL_BUDGET_ENV`] payload that fails to decode is
/// refused rather than dropped — see [`prompt_runtime_from_env`].
pub fn prompt_runtime_extension_for_env() -> Result<Option<Arc<dyn NativeExtension>>, String> {
    prompt_runtime_extension_from(&|key| std::env::var(key).ok())
}

/// The env-injected form of [`prompt_runtime_extension_for_env`] — the whole decision as a pure
/// function of a lookup, so it is testable without mutating process-global environment state.
///
/// # Errors
/// As [`prompt_runtime_extension_for_env`].
pub fn prompt_runtime_extension_from(
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Arc<dyn NativeExtension>>, String> {
    Ok(prompt_runtime_from_env(get)?.map(|runtime| Arc::new(runtime) as Arc<dyn NativeExtension>))
}

/// [`prompt_runtime_extension_from`] before the trait object is erased — the same decision, typed,
/// so a caller (and a test) can inspect which halves actually armed.
///
/// # Errors
/// CFG-080: pi's `decodeToolBudgetEnv` THROWS out of the registration function
/// (`subagent-prompt-runtime.ts:693`, `tool-budget.ts:74-80` @v0.64.0), so an undecodable tool
/// budget is a construction failure here too — the message is pi's own, and no runtime is built.
pub fn prompt_runtime_from_env(
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<SubagentPromptRuntime>, String> {
    let non_empty = |key: &str| get(key).filter(|value| !value.trim().is_empty());

    let capture = non_empty(STRUCTURED_OUTPUT_CAPTURE_ENV);
    let tool = match (&capture, non_empty(STRUCTURED_OUTPUT_SCHEMA_ENV)) {
        (Some(capture), Some(schema_path)) => std::fs::read(&schema_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .map(|schema| Arc::new(StructuredOutputTool::new(schema, PathBuf::from(capture)))),
        _ => None,
    };

    let inherit_project_context = read_boolean_env(get, INHERIT_PROJECT_CONTEXT_ENV);
    let inherit_skills = read_boolean_env(get, INHERIT_SKILLS_ENV);
    let fanout_child = read_boolean_env(get, FANOUT_CHILD_ENV);
    // pi `:333`: all three undefined => no rewrite at all.
    let rewrite =
        (inherit_project_context.is_some() || inherit_skills.is_some() || fanout_child.is_some())
            .then(|| PromptRewriteOptions {
                inherit_project_context: inherit_project_context.unwrap_or(true),
                inherit_skills: inherit_skills.unwrap_or(true),
                fanout_child: fanout_child == Some(true),
                // pi `:111` gates the appended instruction on the CAPTURE var alone.
                structured_output: capture.is_some(),
            });

    // pi `registerNativeSupervisorClient` (`subagent-prompt-runtime.ts:240` →
    // `native-supervisor-channel.ts:294-311`): a child with a resolvable supervisor channel gets a
    // `contact_supervisor` that needs no broker, no socket and no intercom opt-in. See
    // [`crate::native_supervisor::native_child_client_should_register`] for why the second term
    // stands in for upstream's `!hasTool(pi, "contact_supervisor")`.
    let agent_dir =
        crate::native_supervisor::intercom_agent_dir_from(get, std::env::current_dir().ok());
    let child_metadata = crate::native_supervisor::read_child_metadata_from(get).filter(|_| {
        crate::native_supervisor::native_child_client_should_register_from(get, &agent_dir)
    });
    let supervisor_tool = child_metadata.clone().map(|metadata| {
        Arc::new(crate::native_supervisor::NativeContactSupervisorTool::new(
            metadata,
        ))
    });
    // G106, upstream's SECOND child registration (`native-supervisor-channel.ts:305-321`), layered
    // ON TOP of `contact_supervisor` exactly as `registerNativeSupervisorFallbackOnce` layers it
    // (`subagent-prompt-runtime.ts:501-506`): the same channel under the bare name `intercom`,
    // registered only when this agent's own declared `tools:` allowlist asked for a tool by that
    // name (`:513`). A plain child gets `contact_supervisor` alone, as upstream.
    let intercom_fallback = child_metadata
        .filter(|_| {
            crate::native_supervisor::native_child_intercom_fallback_should_register(
                get, &agent_dir,
            )
        })
        .map(|metadata| {
            Arc::new(crate::native_supervisor::NativeChildIntercomTool::new(
                metadata,
            ))
        });

    // pi `:693` @v0.64.0: `registerToolBudget(pi, decodeToolBudgetEnv(process.env[TOOL_BUDGET_ENV],
    // { allowZero: process.env[TOOL_BUDGET_ZERO_AUTH_ENV] === "1" }))` — CFG-067: the zero-budget
    // authorisation is read from the same env the budget arrives in, so a `hard: 0` payload is
    // honoured only when the parent said so and rejected otherwise.
    //
    // CFG-080: the rejection PROPAGATES. `decodeToolBudgetEnv` throws (`tool-budget.ts:74-80`
    // @v0.64.0) out of `registerSubagentPromptRuntime`, and pi's loader catches that throw,
    // `load.discard()`s every registration the factory had already made and records
    // `Failed to load extension: …` (`pi/packages/coding-agent/src/core/extensions/loader.ts:545-587`
    // @v0.84.4) — so the one payload the `hard: 0` authorisation exists to gate can never reach a
    // running child. Dropping it with a `tracing::warn!` (this crate until CFG-080) inverted that:
    // an UNAUTHORISED `{"hard":0}` was exactly such a decode error, and it disabled the budget
    // entirely instead of enforcing it.
    //
    // **[CYRUP-DELTA] on blast radius, in the safe direction.** pi loses only this extension and
    // the child keeps running; cyrup has no per-extension quarantine at its native-extension
    // attach point, so the error travels out of the launch path and the child exits before its
    // first turn (`crates/cyrup/src/session_launch.rs`). Both refuse to run a budgeted child with
    // its budget silently removed; cyrup additionally refuses to run it with the rest of the
    // subagent runtime missing, which for this crate's children is not a survivable state anyway.
    //
    // The radius is wider than a subagent child, and deliberately so: this decode runs before the
    // inertness check below, so `attach_native_extensions` is unconditional and a stray or stale
    // `CYRUP_SUBAGENT_TOOL_BUDGET` refuses a TOP-LEVEL interactive launch too, where pi would
    // merely have lost one extension. Only the parent writes that variable, so a value present in
    // a top-level environment is already evidence the environment is not the one this process was
    // handed — which is the case this arm exists to refuse, not one to make an exception for.
    let tool_budget = crate::exec::tool_budget::decode_tool_budget_env(
        get(crate::exec::tool_budget::TOOL_BUDGET_ENV).as_deref(),
        crate::exec::tool_budget::HardMinimum::from_env(get),
    )?;

    // G90 / pi `:194-195`: `const steerInbox = process.env[SUBAGENT_STEER_INBOX_ENV]?.trim(); if
    // (!steerInbox) return;`. A blank value is the same as unset, which is what makes the trim
    // load-bearing rather than cosmetic — an empty path would otherwise resolve to the process cwd
    // and a poller would drain unrelated files from it.
    let steer_inbox = non_empty(STEER_INBOX_ENV).map(PathBuf::from);

    // SUBA-049 / pi `:334-337`: the return path, read with the same trim-and-treat-blank-as-unset
    // rule as the inbox, and each independently optional exactly as upstream's two `?.trim()`
    // reads are. `childIndex` defaults to 0 rather than being rejected: upstream guards every write
    // with `Number.isInteger(childIndex) && childIndex >= 0` and silently skips otherwise, and a
    // single top-level run's only child IS index 0 — so a missing var means "the one child",
    // not "no acknowledgment".
    let steer_ack_dir = non_empty(STEER_ACK_DIR_ENV).map(PathBuf::from);
    let steer_capability_path = non_empty(STEER_CAPABILITY_ENV).map(PathBuf::from);
    let steer_child_index = non_empty(crate::spawn::nested_events::CHILD_INDEX_ENV)
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(0);

    // pi `registerChildWatchdog(pi)` (`subagent-prompt-runtime.ts:477`), which reads
    // `process.env[CHILD_WATCHDOG_CONFIG_ENV]` itself. `None` for every child the orchestrator did
    // not arm, which is the default.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let raw_watchdog_config = non_empty(crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV);
    let services: Arc<std::sync::Mutex<Option<Arc<dyn cyrup_ext::host::HostServices>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let sink_services = Arc::clone(&services);
    // `review: createMainWatchdogReview(() => currentContext, { getThinkingLevel: () =>
    // pi.getThinkingLevel() })` (`register-child.ts:77`). Passing `None` here left the child on
    // `InertWatchdogReview` — a runtime that resolves no model, calls nothing and reports every
    // boundary clean, so an armed child was watched in name only. The review is built only when
    // the child is actually armed, since constructing it opens the process's `auth.json`.
    let child_review = raw_watchdog_config
        .as_ref()
        .map(|_| child_watchdog_review(&cwd, &services));
    let watchdog = crate::watchdog::register_child::register_child_watchdog(
        raw_watchdog_config.as_deref(),
        &cwd,
        Arc::new(move || {
            sink_services
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }),
        child_review,
        crate::watchdog::register_child::stdout_status_sink(),
    );

    // pi `registerPermissionGate(pi)` (`subagent-prompt-runtime.ts:475`), which reads
    // `process.env[PERMISSION_POLICY_ENV]` itself (`:285`).
    let permission_policy = non_empty(crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV);
    let permission_audit_path =
        non_empty(crate::watchdog::permission_arbiter::PERMISSION_AUDIT_PATH_ENV)
            .map(PathBuf::from);

    let runtime = SubagentPromptRuntime::from_parts(tool, rewrite, fanout_child == Some(true))
        .with_supervisor_tool(supervisor_tool)
        .with_intercom_fallback(intercom_fallback)
        .with_tool_budget(tool_budget)
        .with_steering_channel(
            steer_inbox,
            steer_ack_dir,
            steer_capability_path,
            steer_child_index,
        )
        .with_permission_gate(
            permission_policy.as_deref(),
            raw_watchdog_config,
            permission_audit_path,
            // `createWatchdogPermissionArbiter()` with no `streamFn` override
            // (`permission-arbiter.ts:145`). cyrup binds no in-process model turn here for the
            // same reason [`crate::watchdog::review::NoTurnReviewAgent`] exists, and the result is
            // the fail-closed one: an `ask` denies as `malformed` rather than approving silently.
            Arc::new(crate::watchdog::permission_arbiter::NoDecisionPermissionAgent),
        )
        .with_watchdog(watchdog, services)
        // SUBA-045 / pi `refreshChildToolDiagnostic` (`subagent-prompt-runtime.ts:98-103`), armed
        // from the pair of env vars the parent writes at `pi-args.ts:610-616`.
        .with_tool_diagnostic(get);

    if runtime.is_inert() {
        return Ok(None);
    }
    Ok(Some(runtime))
}

/// `createMainWatchdogReview(() => currentContext, { getThinkingLevel: () => pi.getThinkingLevel()
/// })` for the CHILD role (`register-child.ts:77`) — the same review the orchestrator binds, over
/// the same late-bound capability slot the child's warning sink already resolves through.
fn child_watchdog_review(
    cwd: &Path,
    services: &Arc<std::sync::Mutex<Option<Arc<dyn cyrup_ext::host::HostServices>>>>,
) -> Arc<dyn crate::watchdog::runtime::WatchdogReview> {
    let registry: Arc<dyn crate::watchdog::model_selection::WatchdogModelRegistry> = Arc::new(
        crate::watchdog::model_selection::BuiltinWatchdogModelRegistry::new(
            crate::watchdog::register_main::watchdog_config_dirs().as_ref(),
        ),
    );
    let session_registry = Arc::clone(&registry);
    let session_services = Arc::clone(services);
    Arc::new(
        crate::watchdog::review::MainWatchdogReview::new(
            registry,
            Arc::new(crate::watchdog::review::AmbientReviewAuth),
            Arc::new(crate::watchdog::review::NoTurnReviewAgent),
            cwd.to_path_buf(),
        )
        .with_session_context(Arc::new(move || {
            let services = session_services
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()?;
            let model = services.current_model().as_deref().and_then(|model| {
                let info = crate::watchdog::register_main::watchdog_model_info(model)?;
                Some(
                    session_registry
                        .find(&info.provider, &info.id)
                        .unwrap_or(info),
                )
            });
            Some(crate::watchdog::review::WatchdogSessionContext {
                model,
                thinking_level: services.thinking_level(),
            })
        })),
    )
}

#[cfg(test)]
mod tool_budget_runtime_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use cyrup_core::ToolCallId;
    use cyrup_ext::native::ExtMode;

    fn budget(json: &str) -> crate::discovery::types::ResolvedToolBudget {
        crate::exec::tool_budget::validate_tool_budget_config(
            Some(&serde_json::from_str(json).expect("valid JSON")),
            "toolBudget",
        )
        .expect("valid budget")
        .expect("some")
    }

    fn ctx() -> HostCtx {
        HostCtx::event(ExtMode::Json, false, PathBuf::from("/tmp"))
    }

    fn call(n: u32, name: &str) -> HostEvent {
        HostEvent::ToolCall {
            call_id: ToolCallId::from(format!("call-{n}")),
            name: name.to_string(),
            input: serde_json::json!({}),
        }
    }

    fn result_of(n: u32, name: &str) -> HostEvent {
        HostEvent::ToolResult {
            call_id: ToolCallId::from(format!("call-{n}")),
            name: name.to_string(),
            input: serde_json::json!({}),
            content: vec![cyrup_core::Content::Text {
                text: "ok".into(),
                text_signature: None,
            }],
            details: None,
            is_error: false,
            usage: None,
            terminate: cyrup_core::TerminateHint::Unspecified,
        }
    }

    /// The USER ACTION end to end, as a full interleaved SEQUENCE (not one block in isolation): an
    /// agent declares `toolBudget: {"hard": 2, "soft": 1}`, the parent encodes it into the child's
    /// env, the child builds its runtime FROM that env, and then the child runs five tool calls.
    /// Call 1 earns the soft nudge on its result; calls 3+ are hard-blocked.
    #[tokio::test]
    async fn a_budget_shipped_in_the_env_nudges_once_then_blocks_every_later_browsing_call() {
        let encoded = crate::exec::tool_budget::encode_tool_budget_env(Some(&budget(
            "{\"hard\": 2, \"soft\": 1}",
        )))
        .expect("encodes");
        let ext = prompt_runtime_extension_from(&|key| {
            if key == crate::exec::tool_budget::TOOL_BUDGET_ENV {
                Some(encoded.clone())
            } else {
                None
            }
        })
        .expect("a valid budget decodes")
        .expect("a budget alone is enough to build the child runtime");

        let ctx = ctx();
        let mut nudges = Vec::new();
        let mut blocked = Vec::new();
        for n in 1..=5u32 {
            match ext.on_event(&call(n, "read"), &ctx).await {
                HookOutcome::Block { reason, .. } => blocked.push((n, reason.unwrap_or_default())),
                HookOutcome::Noop => {}
                other => panic!("unexpected tool_call outcome at {n}: {other:?}"),
            }
            if let HookOutcome::Mutate(EventPatch::ToolResult { content, .. }) =
                ext.on_event(&result_of(n, "read"), &ctx).await
                && let Some(content) = content
                && let Some(cyrup_core::Content::Text { text, .. }) = content.last()
            {
                nudges.push((n, text.clone()));
            }
        }

        assert_eq!(
            nudges.len(),
            1,
            "the soft nudge fires exactly once: {nudges:?}"
        );
        assert_eq!(
            nudges[0].0, 1,
            "it rides the result of the call that crossed soft"
        );
        assert!(
            nudges[0]
                .1
                .starts_with("Tool budget soft limit reached after 1 tool call (soft 1, hard 2)."),
            "nudge text: {}",
            nudges[0].1
        );
        assert_eq!(
            blocked.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            vec![3, 4, 5],
            "calls 1 and 2 are within the hard budget; every later one is refused"
        );
        assert!(
            blocked[0]
                .1
                .starts_with("Tool budget hard limit reached after 3 tool calls (hard 2). The 'read' tool is blocked"),
            "block reason: {}",
            blocked[0].1
        );
    }

    /// The default block list is pi's browsing tools only — an over-budget child can still `bash`
    /// and `write` its way to a finished answer.
    #[tokio::test]
    async fn an_over_budget_child_can_still_use_a_non_blocked_tool() {
        let ext = SubagentPromptRuntime::from_parts(None, None, false)
            .with_tool_budget(Some(budget("{\"hard\": 1}")));
        let ctx = ctx();
        assert!(matches!(
            ext.on_event(&call(1, "read"), &ctx).await,
            HookOutcome::Noop
        ));
        assert!(matches!(
            ext.on_event(&call(2, "bash"), &ctx).await,
            HookOutcome::Noop
        ));
        assert!(matches!(
            ext.on_event(&call(3, "read"), &ctx).await,
            HookOutcome::Block { .. }
        ));
    }

    /// CFG-067 / pi `subagent-prompt-runtime.ts:693` @v0.64.0: `decodeToolBudgetEnv(env, {
    /// allowZero: env[TOOL_BUDGET_ZERO_AUTH_ENV] === "1" })`. THE USER ACTION end to end: the
    /// parent ships `{"hard": 0}` — "this child may make no browsing calls at all" — and the
    /// authorisation flag; the child's very first `read` is refused. Without the flag the same
    /// payload is a decode error, and since CFG-080 that error REFUSES the runtime instead of
    /// dropping the budget — the unauthorised half is pinned by
    /// [`an_unauthorised_zero_budget_refuses_the_runtime_instead_of_running_unbudgeted`].
    #[tokio::test]
    async fn a_zero_budget_is_honoured_only_with_the_parents_authorisation() {
        let encoded = "{\"hard\":0}".to_string();
        let authorised = prompt_runtime_extension_from(&move |key| {
            if key == crate::exec::tool_budget::TOOL_BUDGET_ENV {
                Some(encoded.clone())
            } else if key == crate::exec::tool_budget::TOOL_BUDGET_ZERO_AUTH_ENV {
                Some("1".to_string())
            } else {
                None
            }
        })
        .expect("an authorised zero budget decodes")
        .expect("an authorised zero budget builds the child runtime");
        let ctx = ctx();
        match authorised.on_event(&call(1, "read"), &ctx).await {
            HookOutcome::Block { reason, .. } => assert!(
                reason
                    .unwrap_or_default()
                    .starts_with("Tool budget hard limit reached after 1 tool call (hard 0)."),
            ),
            other => panic!("the first browsing call must be refused under hard 0: {other:?}"),
        }
    }

    /// CFG-080 — THE USER ACTION: a `{"hard":0}` budget reaches the child WITHOUT the parent's
    /// `CYRUP_SUBAGENT_TOOL_BUDGET_ZERO_AUTH=1`, i.e. the exact payload that authorisation exists
    /// to gate. pi's `decodeToolBudgetEnv` throws that validation message out of
    /// `registerSubagentPromptRuntime` (`subagent-prompt-runtime.ts:693`, `tool-budget.ts:74-80`
    /// @v0.64.0) and its loader discards every registration the factory made
    /// (`pi/packages/coding-agent/src/core/extensions/loader.ts:545-587` @v0.84.4), so the child
    /// can never run with the budget silently removed. Before CFG-080 this resolver logged a
    /// `tracing::warn!` and returned a runtime with `tool_budget: None` — the child ran with
    /// UNLIMITED tool calls, the fail-OPEN inverse of upstream. Every decode failure is refused
    /// the same way, matching upstream's single `throw`.
    #[test]
    fn an_unauthorised_zero_budget_refuses_the_runtime_instead_of_running_unbudgeted() {
        let refused = |payload: &str, expected: &str| {
            let value = payload.to_string();
            let Err(error) = prompt_runtime_from_env(&move |key| {
                (key == crate::exec::tool_budget::TOOL_BUDGET_ENV).then(|| value.clone())
            }) else {
                panic!("an undecodable tool budget must refuse the runtime: {payload:?}");
            };
            assert!(
                error.contains(expected),
                "expected {expected:?} in {error:?}"
            );
        };

        // pi's own message, with `minimumHard` at its default 1 because the authorisation is
        // absent (`tool-budget.ts:20-24`).
        refused(
            "{\"hard\":0}",
            "CYRUP_SUBAGENT_TOOL_BUDGET.hard must be an integer >= 1.",
        );
        // The same refusal for a payload that is not even JSON — pi's `JSON.parse` throws from
        // the same call.
        refused("not json", "is not valid JSON");
        // And for a semantically invalid one.
        refused(
            "{\"hard\":2,\"soft\":5}",
            "CYRUP_SUBAGENT_TOOL_BUDGET.soft must be <= CYRUP_SUBAGENT_TOOL_BUDGET.hard.",
        );

        // The authorised payload is NOT refused — the gate is what separates them.
        assert!(
            prompt_runtime_from_env(&|key| match key {
                k if k == crate::exec::tool_budget::TOOL_BUDGET_ENV =>
                    Some("{\"hard\":0}".to_string()),
                k if k == crate::exec::tool_budget::TOOL_BUDGET_ZERO_AUTH_ENV =>
                    Some("1".to_string()),
                _ => None,
            })
            .expect("an authorised zero budget decodes")
            .is_some_and(|runtime| !runtime.is_inert()),
            "the authorised zero budget still builds an armed child runtime"
        );
    }

    /// No budget in the env => no `tool_call` subscription and no interference at all.
    #[tokio::test]
    async fn a_child_with_no_budget_never_blocks_a_tool() {
        let ext = SubagentPromptRuntime::from_parts(None, None, false).with_tool_budget(None);
        let ctx = ctx();
        for n in 1..=10u32 {
            assert!(matches!(
                ext.on_event(&call(n, "read"), &ctx).await,
                HookOutcome::Noop
            ));
        }
        assert!(
            prompt_runtime_extension_from(&|_| None)
                .expect("no tool budget in this env, so the runtime always builds")
                .is_none()
        );
    }
}

/// pi `registerPermissionGate` (`subagent-prompt-runtime.ts:281-305`, installed at `:475`) driven
/// through the REAL extension surface: the env resolver builds it, `init` subscribes `tool_call`,
/// and `on_event` decides.
#[cfg(test)]
mod permission_gate_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::watchdog::permission_arbiter::{PERMISSION_AUDIT_PATH_ENV, PERMISSION_POLICY_ENV};
    use cyrup_core::ToolCallId;
    use cyrup_ext::native::ExtMode;

    fn ctx() -> HostCtx {
        HostCtx::event(ExtMode::Json, false, PathBuf::from("/tmp"))
    }

    fn call(name: &str) -> HostEvent {
        HostEvent::ToolCall {
            call_id: ToolCallId::from("call-1"),
            name: name.to_string(),
            input: serde_json::json!({ "path": "a.rs", "apiKey": "SK-ABCDEFGHIJ" }),
        }
    }

    fn env_extension(policy: &str, audit: Option<&str>) -> Arc<dyn NativeExtension> {
        let policy = policy.to_string();
        let audit = audit.map(str::to_string);
        prompt_runtime_extension_from(&move |key| match key {
            PERMISSION_POLICY_ENV => Some(policy.clone()),
            PERMISSION_AUDIT_PATH_ENV => audit.clone(),
            _ => None,
        })
        .expect("no tool budget in this env, so the runtime always builds")
        .expect("a policy alone arms the child runtime")
    }

    /// The USER ACTION end to end: a parent ships `{"write":"deny","read":"allow"}` in the child's
    /// env, and the child refuses `write` while `read` passes — through `on_event`, not through a
    /// direct call to the gate.
    #[tokio::test]
    async fn a_policy_in_the_env_blocks_the_denied_tool_and_passes_the_allowed_one() {
        let ext = env_extension("{\"write\":\"deny\",\"read\":\"allow\"}", None);
        let mut api = InitApi::new();
        ext.init(&mut api).await.expect("init");
        assert!(
            api.subscriptions().contains(EventKind::ToolCall),
            "the gate must subscribe tool_call or it is never consulted"
        );

        let ctx = ctx();
        let HookOutcome::Block { reason, .. } = ext.on_event(&call("write"), &ctx).await else {
            panic!("a denied tool must be blocked");
        };
        assert_eq!(
            reason.as_deref(),
            Some("Blocked by pi-subagents permission rule: 'write' is denied.")
        );
        assert!(matches!(
            ext.on_event(&call("read"), &ctx).await,
            HookOutcome::Noop
        ));
    }

    /// The `ask` tier with no model turn bound: [`NoDecisionPermissionAgent`] reaches no decision,
    /// which the arbiter reports as `malformed` and the gate turns into a BLOCK. Fail-closed is the
    /// whole point — a child has no human to ask.
    #[tokio::test]
    async fn an_ask_tier_tool_fails_closed_and_writes_both_audit_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let audit = dir.path().join("permission-audit.jsonl");
        let ext = env_extension("{\"write\":\"ask\"}", Some(&audit.display().to_string()));
        let HookOutcome::Block { reason, .. } = ext.on_event(&call("write"), &ctx()).await else {
            panic!("an unanswerable ask must fail closed");
        };
        assert_eq!(
            reason.as_deref(),
            Some(
                "Blocked by pi-subagents permission rule: Watchdog permission arbiter is \
                 unavailable because the child watchdog is disabled."
            )
        );
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&audit)
            .expect("the audit file was written")
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["type"], serde_json::json!("permission.request"));
        assert_eq!(lines[1]["approved"], serde_json::json!(false));
        // The audited preview is redacted — and `SK-ABCDEFGHIJ` only redacts because the regex is
        // case-insensitive.
        let preview = lines[0]["preview"].as_str().expect("preview");
        assert!(preview.contains("[redacted]"), "{preview}");
        assert!(!preview.contains("ABCDEFGHIJ"), "{preview}");
    }

    /// `permissionDecision` (`permissions.ts:46-49`): `bash` and the four internal coordination
    /// tools are allowed no matter what the rules say, so a parent cannot strand a child.
    #[tokio::test]
    async fn bash_and_the_internal_coordination_tools_are_never_gated() {
        // The rules a parent could still ship for them (validation refuses to record these, but a
        // parent on another version could).
        let ext = env_extension("{\"write\":\"deny\",\"read\":\"deny\"}", None);
        let ctx = ctx();
        for name in [
            "bash",
            "contact_supervisor",
            "intercom",
            "subagent_wait",
            "structured_output",
        ] {
            assert!(
                matches!(ext.on_event(&call(name), &ctx).await, HookOutcome::Noop),
                "{name} must never be gated"
            );
        }
    }

    /// [CYRUP-DELTA] an undecodable policy blocks everything rather than degrading to "no policy".
    #[tokio::test]
    async fn an_invalid_policy_blocks_every_gated_tool_instead_of_failing_open() {
        let ext = env_extension("{\"write\":\"maybe\"}", None);
        let HookOutcome::Block { reason, .. } = ext.on_event(&call("write"), &ctx()).await else {
            panic!("an invalid policy must not fail open");
        };
        let reason = reason.expect("a reason");
        assert!(
            reason.contains("the permission policy is invalid"),
            "{reason}"
        );
        assert!(reason.contains("must be allow, ask, or deny"), "{reason}");
    }

    /// No policy in the env => no gate, no `tool_call` subscription, and — with nothing else set —
    /// no extension at all, exactly as `registerPermissionGate`'s early return (`:286`).
    #[tokio::test]
    async fn a_child_with_no_policy_installs_no_gate() {
        assert!(
            prompt_runtime_extension_from(&|_| None)
                .expect("no tool budget in this env, so the runtime always builds")
                .is_none()
        );
        let runtime = SubagentPromptRuntime::from_parts(None, None, false).with_permission_gate(
            None,
            None,
            None,
            Arc::new(crate::watchdog::permission_arbiter::NoDecisionPermissionAgent),
        );
        assert!(runtime.permission_gate().is_none());
        assert!(runtime.is_inert());
        let mut api = InitApi::new();
        runtime.init(&mut api).await.expect("init");
        assert!(!api.subscriptions().contains(EventKind::ToolCall));
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn tool(schema: serde_json::Value, path: PathBuf) -> StructuredOutputTool {
        StructuredOutputTool::new(schema, path)
    }

    /// SUBA-049 (review) — a DROPPED `flush` future must not latch the re-entrancy guard.
    ///
    /// Upstream cannot have this bug: its `flush` is a synchronous `(): void` whose body is wrapped
    /// in `try { … } finally { flushing = false; }` (`subagent-prompt-runtime.ts:381-413`). cyrup's
    /// port is `async` and awaits at `consume_steer_requests_from_dir`, at every `acknowledge` and
    /// at the write-back, and it is driven both from the poll task and from the turn-lifecycle event
    /// handlers — so the future genuinely can be dropped mid-body.
    ///
    /// **Red before the fix:** `flush` set `state.flushing = true` before its first `.await` and
    /// cleared it only with a trailing assignment on the fall-through path, reproducing upstream's
    /// `try` but NOT its `finally`. This test parks `flush` at its first await and drops it; pre-fix
    /// `flushing` is still `true` afterwards, the second assertion fails, and — the consequence that
    /// matters — the next flush takes the `disposed || flushing` early return, so the queued request
    /// is never consumed and `remaining_after` stays 1 forever. Post-fix `FlushGuard::drop` clears
    /// the latch and the second flush drains the inbox.
    ///
    /// **Why this builds its own runtime.** `flush`'s first await is
    /// `tokio::fs::read_dir` (`background/control.rs:1021`), which is `asyncify` →
    /// `spawn_blocking(..).await` (tokio `fs/read_dir.rs:31-41`, `fs/mod.rs:312-324`). The blocking
    /// pool is real OS threads under EVERY runtime flavor (`runtime/builder.rs:1676` builds one for
    /// `new_current_thread` too) and `spawn_blocking` dispatches at call time, so awaiting that
    /// `JoinHandle` is a RACE, not a yield point: when the pool thread finishes first the poll
    /// returns `Ready`, and — since `next_entry` is served from `read_dir`'s own 32-entry buffer and
    /// `acknowledge` returns at its `ack_dir: None` guard before any await — the whole body can
    /// complete in one poll, leaving no mid-body drop to test. That is the intermittent this shape
    /// removes: with the pool capped at one thread and that thread occupied,
    /// `BlockingPool::spawn_task` can only QUEUE the read (`runtime/blocking/pool.rs:406-415`), so
    /// the first poll MUST park at the intended await.
    ///
    /// Measured before this shape: 25/25 passes in isolation but 11 failures in 72 runs at 12x
    /// load, every one at the `Poll::Pending` precondition and never at the latch assertion or the
    /// drain — the latch is armed at `FlushGuard` construction, before any await, so no drop point
    /// can strand it. The flake was the scheduler being assumed, not the behaviour under test.
    #[test]
    fn a_dropped_flush_future_does_not_wedge_the_steering_inbox() {
        use std::sync::mpsc;
        use std::task::{Context, Poll, Waker};

        let rt = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .expect("runtime");

        rt.block_on(async {
            let temp = tempfile::tempdir().expect("tempdir");
            let dir = temp.path().join("steer-inbox");
            std::fs::create_dir_all(&dir).expect("create inbox");

            let request = crate::background::control::SteerRequest {
                kind: "steer".to_string(),
                id: "req-1".to_string(),
                ts: 1,
                message: "look at the retry path".to_string(),
                mode: None,
                target_index: None,
                source: None,
            };
            // Written BEFORE the gate goes up: this itself needs the blocking pool
            // (`control.rs:799` → `tokio::fs::create_dir_all`), and it is what forces the pool's
            // single thread into existence.
            crate::background::control::write_steer_request_to_dir(&dir, &request)
                .await
                .expect("write request");

            let inbox = Arc::new(SteeringInbox::new(dir.clone(), None, None, 0));
            // `can_steer` is what the turn-lifecycle events set; without it `flush` returns before
            // the latch is even reached and this test would prove nothing.
            inbox.state.lock().expect("not poisoned").can_steer = true;

            // ---- the gate: occupy the pool's only thread ------------------------------------
            // `spawn_blocking` dispatches at CALL time, so once `started_rx.recv()` has returned
            // the single pool thread is provably inside this closure and every later blocking task
            // is queued, never run.
            let (started_tx, started_rx) = mpsc::channel::<()>();
            let (release_tx, release_rx) = mpsc::channel::<()>();
            let gate = tokio::task::spawn_blocking(move || {
                started_tx.send(()).expect("gate handshake");
                let _ = release_rx.recv();
            });
            started_rx
                .recv()
                .expect("the blocking pool's only thread is occupied");

            {
                let fut = inbox.flush();
                let mut fut = std::pin::pin!(fut);
                let mut cx = Context::from_waker(Waker::noop());
                assert!(
                    matches!(fut.as_mut().poll(&mut cx), Poll::Pending),
                    "with the one-thread blocking pool gated, `flush`'s first await \
                     (`tokio::fs::read_dir` → `asyncify` → `spawn_blocking`) cannot have \
                     completed, so this poll must park mid-body"
                );
                assert!(
                    inbox.state.lock().expect("not poisoned").flushing,
                    "the latch must be held at the drop point, or the test is not testing anything"
                );
            } // `fut` dropped here, mid-body, parked at `read_dir`.

            assert!(
                !inbox.state.lock().expect("not poisoned").flushing,
                "a dropped flush must release the re-entrancy latch (pi's `finally`)"
            );

            // Reopen the gate. The `read_dir` task the dropped future left queued runs first and is
            // inert: it only opens the directory and buffers entries — removal happens later, at
            // `control.rs:1042`, which that future never reached.
            drop(release_tx);
            gate.await.expect("gate task");

            // The consequence: a later flush still drains the inbox. No services are bound, so each
            // request is acknowledged `failed` and consumed rather than injected — either way it
            // must LEAVE the directory, which a wedged latch would prevent.
            inbox.flush().await;
            let remaining_after = std::fs::read_dir(&dir)
                .expect("read inbox")
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                .count();
            assert_eq!(
                remaining_after, 0,
                "the request must be consumed by the flush that follows a dropped one"
            );
        });
    }

    /// SUBA-045 — a [`cyrup_ext::host::HostServices`] double that answers only `all_tool_names`,
    /// which is pi's `pi.getAllTools()`. Every other capability keeps the trait's default.
    struct RegistryHost(Option<Vec<String>>);
    impl cyrup_ext::host::HostServices for RegistryHost {
        fn all_tool_names(&self) -> Option<Vec<String>> {
            self.0.clone()
        }
    }

    /// SUBA-045 — the whole child-side hop: the parent's two env vars arm the diagnostic, an
    /// `agent_start` diffs the required list against the LIVE registry, and the file is written
    /// only when something is genuinely absent.
    ///
    /// The three legs are ordered so none can pass vacuously: the missing leg proves the file
    /// appears, the present leg then proves the SAME path is cleaned up (the stale-file case), and
    /// the un-armed leg proves the whole thing stays off without the env pair.
    #[tokio::test]
    async fn agent_start_writes_the_tool_diagnostic_only_for_a_genuinely_missing_tool() {
        use crate::exec::tool_availability as ta;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = ta::tool_diagnostic_path_in(dir.path());
        let path_string = path.display().to_string();

        let env = |armed: bool| {
            let path_string = path_string.clone();
            move |key: &str| -> Option<String> {
                match key {
                    k if k == ta::CHILD_TOOL_DIAGNOSTIC_PATH_ENV && armed => {
                        Some(path_string.clone())
                    }
                    k if k == crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS => {
                        Some(r#"["read","mcp__srv__gone"]"#.to_string())
                    }
                    k if k == crate::spawn::intercom_target::ENV_CHILD_AGENT => {
                        Some("researcher".to_string())
                    }
                    k if k == ta::MCP_DIRECT_CHILD_TOOLS_ENV => {
                        Some(r#"["mcp__srv__gone"]"#.to_string())
                    }
                    _ => None,
                }
            }
        };

        async fn fire(env: &dyn Fn(&str) -> Option<String>, registry: Option<Vec<String>>) {
            let runtime =
                SubagentPromptRuntime::from_parts(None, None, false).with_tool_diagnostic(env);
            runtime.set_host_services(Arc::new(RegistryHost(registry)));
            runtime
                .on_event(
                    &HostEvent::AgentStart,
                    &HostCtx::event(cyrup_ext::native::ExtMode::Json, false, PathBuf::from(".")),
                )
                .await;
        }

        // (1) The MCP tool the host never registered: the file lands, names it, and says so.
        fire(&env(true), Some(vec!["read".to_string()])).await;
        let reported = ta::read_child_tool_diagnostic_error(Some(&path))
            .expect("a missing tool must produce a reportable diagnostic");
        assert!(
            reported.starts_with(
                "Agent 'researcher' requested unavailable child tools: mcp__srv__gone."
            ),
            "{reported}"
        );
        assert!(
            reported.contains("host/pi-mcp-adapter registration problem"),
            "the MCP-direct half must be attributed, not folded into the generic line: {reported}"
        );

        // (2) Same path, everything present now — upstream's `rmSync(..., { force: true })`.
        fire(
            &env(true),
            Some(vec!["read".to_string(), "mcp__srv__gone".to_string()]),
        )
        .await;
        assert!(
            !path.exists(),
            "a healthy child must leave NO diagnostic behind, or the next attempt inherits it"
        );

        // (3) Un-armed (the parent wrote no diagnostic path): nothing is written at all, even
        //     though the required list and an empty-ish registry would otherwise report `read` and
        //     `mcp__srv__gone` as missing.
        fire(&env(false), Some(Vec::new())).await;
        assert!(
            !path.exists(),
            "the handshake must stay off without its path env"
        );
    }

    /// SUBA-045 — a backend that cannot answer `all_tool_names` (no live session bound) is "no
    /// snapshot", not "no tools". Writing a diagnostic here would report EVERY required tool as
    /// missing, which is the loudest possible way to be wrong.
    #[tokio::test]
    async fn an_unanswerable_registry_writes_no_diagnostic_at_all() {
        use crate::exec::tool_availability as ta;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = ta::tool_diagnostic_path_in(dir.path());
        let path_string = path.display().to_string();
        let env = move |key: &str| -> Option<String> {
            if key == ta::CHILD_TOOL_DIAGNOSTIC_PATH_ENV {
                Some(path_string.clone())
            } else if key == crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS {
                Some(r#"["read"]"#.to_string())
            } else {
                None
            }
        };
        let runtime =
            SubagentPromptRuntime::from_parts(None, None, false).with_tool_diagnostic(&env);
        runtime.set_host_services(Arc::new(RegistryHost(None)));
        runtime
            .on_event(
                &HostEvent::AgentStart,
                &HostCtx::event(cyrup_ext::native::ExtMode::Json, false, PathBuf::from(".")),
            )
            .await;
        assert!(!path.exists());
    }

    /// pi nests the caller's schema under `value` rather than exposing it at the top level
    /// (`subagent-prompt-runtime.ts:283-288`), so the model is constrained by the REAL schema.
    #[test]
    fn parameters_nest_the_callers_schema_under_value() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "verdict": { "type": "string" } },
            "required": ["verdict"],
        });
        let t = tool(schema.clone(), PathBuf::from("/tmp/unused.json"));
        let params = t.parameters();

        assert_eq!(params["properties"]["value"], schema);
        assert_eq!(params["required"], serde_json::json!(["value"]));
        assert_eq!(params["additionalProperties"], serde_json::json!(false));
    }

    // ---- G81: `$ref` rewrite (pi `rewriteLocalJsonPointerRefs`, structured-output.ts:23-69) ----

    /// A recursive `$defs` schema is the shape that breaks: nesting it under `value` moves every
    /// definition one level deeper, so `#/$defs/Node` and `#` both stop resolving. Both forms must
    /// be repointed at the wrapper-relative location.
    #[test]
    fn local_json_pointer_refs_are_repointed_under_the_value_wrapper() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "root": { "$ref": "#/$defs/Node" } },
            "required": ["root"],
            "$defs": {
                "Node": {
                    "type": "object",
                    "properties": {
                        "children": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
                        "whole": { "$ref": "#" },
                    },
                },
            },
        });
        let t = tool(schema, PathBuf::from("/tmp/unused.json"));
        let value = &t.parameters()["properties"]["value"];

        assert_eq!(
            value["properties"]["root"]["$ref"],
            serde_json::json!("#/properties/value/$defs/Node"),
            "a `#/`-rooted pointer gains the wrapper prefix"
        );
        assert_eq!(
            value["$defs"]["Node"]["properties"]["children"]["items"]["$ref"],
            serde_json::json!("#/properties/value/$defs/Node"),
            "the walk descends through `$defs` and single-schema `items`"
        );
        assert_eq!(
            value["$defs"]["Node"]["properties"]["whole"]["$ref"],
            serde_json::json!("#/properties/value"),
            "the whole-document pointer `#` becomes the wrapper pointer itself"
        );
    }

    /// Every keyword family pi walks (`SCHEMA_ARRAY_KEYWORDS`, `SCHEMA_SINGLE_KEYWORDS`, the
    /// tuple form of `items`, and the draft-07 `dependencies` union) must be descended into — and
    /// a `dependencies` ARRAY value is a property-name list, not a schema, so it stays verbatim.
    #[test]
    fn every_schema_keyword_family_is_walked() {
        let schema = serde_json::json!({
            "anyOf": [{ "$ref": "#/$defs/A" }],
            "items": [{ "$ref": "#/$defs/A" }, { "$ref": "#" }],
            "additionalProperties": { "$ref": "#/$defs/A" },
            "not": { "$ref": "#/$defs/A" },
            "dependencies": {
                "names": ["alpha", "beta"],
                "shape": { "$ref": "#/$defs/A" },
            },
            "$defs": { "A": { "type": "string" } },
        });
        let t = tool(schema, PathBuf::from("/tmp/unused.json"));
        let value = &t.parameters()["properties"]["value"];

        let expected = serde_json::json!("#/properties/value/$defs/A");
        assert_eq!(value["anyOf"][0]["$ref"], expected);
        assert_eq!(value["items"][0]["$ref"], expected);
        assert_eq!(
            value["items"][1]["$ref"],
            serde_json::json!("#/properties/value")
        );
        assert_eq!(value["additionalProperties"]["$ref"], expected);
        assert_eq!(value["not"]["$ref"], expected);
        assert_eq!(value["dependencies"]["shape"]["$ref"], expected);
        assert_eq!(
            value["dependencies"]["names"],
            serde_json::json!(["alpha", "beta"]),
            "a `dependencies` array is a property-name list, never a subschema"
        );
    }

    /// ALL THREE pointer-bearing keywords are rewritten, not just `$ref` (pi
    /// `structured-output.ts:29`: `for (const keyword of ["$ref", "$dynamicRef", "$recursiveRef"])`).
    ///
    /// `$dynamicRef` (2020-12) and `$recursiveRef` (2019-09) are the recursive-schema keywords, and
    /// they carry exactly the same `#`-relative pointers as `$ref` — so nesting the caller's schema
    /// under `value` invalidates them in exactly the same way. A schema using either one is a
    /// recursive schema by construction, which is precisely the case that breaks loudest: the
    /// pointer resolves against the wrapper, finds no `$defs`, and the whole tool definition is
    /// rejected by a strict validator.
    ///
    /// Both `#`-relative FORMS are asserted per keyword — the bare whole-document `#` and the
    /// `#/`-rooted path — because the two are separate branches of the rewrite.
    #[test]
    fn dynamic_and_recursive_refs_are_repointed_exactly_like_plain_refs() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "viaRef":          { "$ref": "#/$defs/Node" },
                "viaDynamic":      { "$dynamicRef": "#/$defs/Node" },
                "viaRecursive":    { "$recursiveRef": "#/$defs/Node" },
                "wholeDynamic":    { "$dynamicRef": "#" },
                "wholeRecursive":  { "$recursiveRef": "#" },
            },
            "$defs": { "Node": { "type": "object", "$dynamicAnchor": "node" } },
        });
        let t = tool(schema, PathBuf::from("/tmp/unused.json"));
        let value = &t.parameters()["properties"]["value"];

        let rooted = serde_json::json!("#/properties/value/$defs/Node");
        assert_eq!(
            value["properties"]["viaRef"]["$ref"], rooted,
            "`$ref` is the keyword already covered — it anchors the comparison"
        );
        assert_eq!(
            value["properties"]["viaDynamic"]["$dynamicRef"], rooted,
            "`$dynamicRef` carries the same `#/`-rooted pointer and must gain the same prefix"
        );
        assert_eq!(
            value["properties"]["viaRecursive"]["$recursiveRef"], rooted,
            "`$recursiveRef` likewise"
        );

        let whole = serde_json::json!("#/properties/value");
        assert_eq!(
            value["properties"]["wholeDynamic"]["$dynamicRef"], whole,
            "the bare whole-document form is a separate branch, and applies to `$dynamicRef` too"
        );
        assert_eq!(
            value["properties"]["wholeRecursive"]["$recursiveRef"], whole,
            "and to `$recursiveRef`"
        );
    }

    /// The `$id` resource guard applies to all three keywords too — a `$dynamicRef` beneath an
    /// `$id`-bearing subschema resolves against THAT resource, so rewriting it would break a schema
    /// that was already correct. This is the mirror of the rewrite above: proving `$dynamicRef` is
    /// rewritten is only half the behaviour if it is rewritten unconditionally.
    #[test]
    fn dynamic_and_recursive_refs_under_their_own_id_are_left_alone() {
        let schema = serde_json::json!({
            "properties": {
                "embedded": {
                    "$id": "https://example.test/inner.json",
                    "$dynamicRef": "#/$defs/B",
                    "properties": { "deep": { "$recursiveRef": "#" } },
                    "$defs": { "B": { "type": "number" } },
                },
            },
        });
        let t = tool(schema, PathBuf::from("/tmp/unused.json"));
        let value = &t.parameters()["properties"]["value"];

        assert_eq!(
            value["properties"]["embedded"]["$dynamicRef"],
            serde_json::json!("#/$defs/B"),
            "the `$id`-bearing node keeps its own resource-relative `$dynamicRef`"
        );
        assert_eq!(
            value["properties"]["embedded"]["properties"]["deep"]["$recursiveRef"],
            serde_json::json!("#"),
            "and its descendants keep theirs"
        );
    }

    /// pi's `sharesWrapperResource` guard: a subschema with its own `$id` is a NEW schema
    /// resource, so its `#`-relative pointers already resolve against itself — rewriting them
    /// would break a schema that was correct.
    #[test]
    fn a_subschema_with_its_own_id_and_its_descendants_are_left_alone() {
        let schema = serde_json::json!({
            "properties": {
                "outer": { "$ref": "#/$defs/A" },
                "embedded": {
                    "$id": "https://example.test/inner.json",
                    "$ref": "#/$defs/B",
                    "properties": { "deep": { "$ref": "#/$defs/B" } },
                    "$defs": { "B": { "type": "number" } },
                },
            },
            "$defs": { "A": { "type": "string" } },
        });
        let t = tool(schema, PathBuf::from("/tmp/unused.json"));
        let value = &t.parameters()["properties"]["value"];

        assert_eq!(
            value["properties"]["outer"]["$ref"],
            serde_json::json!("#/properties/value/$defs/A"),
            "the wrapper resource is still rewritten"
        );
        assert_eq!(
            value["properties"]["embedded"]["$ref"],
            serde_json::json!("#/$defs/B"),
            "the `$id`-bearing node keeps its own resource-relative pointer"
        );
        assert_eq!(
            value["properties"]["embedded"]["properties"]["deep"]["$ref"],
            serde_json::json!("#/$defs/B"),
            "and so does everything beneath it"
        );
    }

    /// The rewrite is for the ADVERTISED parameters only. Validation of a submitted value runs
    /// against the caller's RAW schema, whose pointers resolve against its own root — so a
    /// `$ref`-bearing schema still accepts a conforming value and still rejects a bad one.
    #[tokio::test]
    async fn validation_still_uses_the_unrewritten_schema() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("output.json");
        let t = tool(
            serde_json::json!({
                "type": "object",
                "properties": { "node": { "$ref": "#/$defs/Node" } },
                "required": ["node"],
                "$defs": {
                    "Node": {
                        "type": "object",
                        "properties": { "label": { "type": "string" } },
                        "required": ["label"],
                    },
                },
            }),
            out.clone(),
        );

        let ok = t
            .execute(
                ToolCallId::from("call-ref-ok"),
                serde_json::json!({ "value": { "node": { "label": "leaf" } } }),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a value conforming to the `$ref`-ed definition is captured");
        assert!(ok.terminate.requested());
        assert!(out.exists(), "the captured value is written for the parent");
    }

    #[tokio::test]
    async fn a_valid_value_is_captured_and_terminates_the_step() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("nested").join("output.json");
        let t = tool(
            serde_json::json!({
                "type": "object",
                "properties": { "verdict": { "type": "string" } },
                "required": ["verdict"],
            }),
            out.clone(),
        );

        let result = t
            .execute(
                ToolCallId::from("call-1"),
                serde_json::json!({ "value": { "verdict": "ship it" } }),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a schema-conforming value is captured");

        assert!(
            result.terminate.requested(),
            "capturing the value terminates the step"
        );
        // The parent reads this file back; the nested dir must have been created for it.
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(written, serde_json::json!({ "verdict": "ship it" }));
    }

    /// An invalid value must NOT write the capture file. If it did, the parent's read-back would
    /// surface a value that never passed validation instead of pi's "missing" hard failure.
    #[tokio::test]
    async fn an_invalid_value_errors_without_writing_the_capture_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("output.json");
        let t = tool(
            serde_json::json!({
                "type": "object",
                "properties": { "verdict": { "type": "string" } },
                "required": ["verdict"],
            }),
            out.clone(),
        );

        let err = t
            .execute(
                ToolCallId::from("call-1"),
                serde_json::json!({ "value": { "wrong": 1 } }),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect_err("a value missing a required property must be refused");

        assert!(
            format!("{err}").contains("Structured output validation failed"),
            "pi's exact wording, got: {err}"
        );
        assert!(
            !out.exists(),
            "an invalid value must leave NO capture file — the parent must still see 'missing'"
        );
    }

    /// A process that is not a subagent child at all — no structured vars, none of the three child
    /// flags — must build NOTHING. This is every top-level `cyrup` session.
    #[test]
    fn a_non_child_process_builds_no_runtime_at_all() {
        assert!(
            prompt_runtime_extension_from(&|_| None)
                .expect("no tool budget in this env, so the runtime always builds")
                .is_none(),
            "an empty environment must not attach the child runtime"
        );
    }

    /// The rewrite half is independent of the structured-output half: a plain child (no declared
    /// schema) still gets the runtime, because it still needs its prompt and context shaped.
    #[test]
    fn the_inherit_flags_alone_build_the_runtime() {
        let env = |key: &str| match key {
            INHERIT_PROJECT_CONTEXT_ENV => Some("0".to_string()),
            INHERIT_SKILLS_ENV => Some("1".to_string()),
            FANOUT_CHILD_ENV => Some("0".to_string()),
            _ => None,
        };
        assert!(
            prompt_runtime_extension_from(&env)
                .expect("no tool budget in this env, so the runtime always builds")
                .is_some(),
            "a child with inherit flags but no schema still needs the prompt/context runtime"
        );
    }

    /// pi `readBooleanEnv` (`:52-56`): absent => `None`; `"0"` => `false`; anything else => `true`.
    #[test]
    fn boolean_env_reads_match_pi_exactly() {
        let val = |v: Option<&str>| {
            let owned = v.map(str::to_string);
            read_boolean_env(&move |_| owned.clone(), "X")
        };
        assert_eq!(val(None), None);
        assert_eq!(val(Some("0")), Some(false));
        assert_eq!(val(Some("1")), Some(true));
        assert_eq!(val(Some("")), Some(true), "only the exact \"0\" is false");
        assert_eq!(val(Some("false")), Some(true), "pi does not parse words");
    }

    fn opts(
        inherit_project_context: bool,
        inherit_skills: bool,
        fanout_child: bool,
    ) -> PromptRewriteOptions {
        PromptRewriteOptions {
            inherit_project_context,
            inherit_skills,
            fanout_child,
            structured_output: false,
        }
    }

    /// A prompt shaped like the real assembler's output (`cyrup-session/src/prompt/builder.rs`
    /// order: body, project context, skills, footer).
    fn assembled_prompt() -> String {
        [
            "You are a coding assistant operating inside cyrup.",
            "",
            "<project_context>",
            "",
            "Project-specific instructions follow.",
            "",
            "<project_instructions path=\"/repo/AGENTS.md\">",
            "NEVER commit to main.",
            "</project_instructions>",
            "",
            "</project_context>",
            "",
            SKILLS_OPEN,
            "<available_skills>",
            "  <skill>",
            "    <name>deploy</name>",
            "  </skill>",
            "</available_skills>",
            "",
            "Current date: 2026-08-07",
        ]
        .join("\n")
    }

    #[test]
    fn inherit_project_context_false_removes_the_project_context_section() {
        let out = rewrite_subagent_prompt(&assembled_prompt(), &opts(false, true, false));
        assert!(
            !out.contains("NEVER commit to main."),
            "inherited AGENTS.md content must be gone"
        );
        assert!(!out.contains(PROJECT_CONTEXT_OPEN));
        assert!(!out.contains(PROJECT_CONTEXT_CLOSE));
        // Everything AROUND the section survives — this is a cut, not a truncation.
        assert!(out.contains("You are a coding assistant operating inside cyrup."));
        assert!(out.contains("Current date: 2026-08-07"));
        assert!(
            out.contains("<name>deploy</name>"),
            "skills were inherited and must remain"
        );
    }

    #[test]
    fn inherit_skills_false_removes_only_the_skills_section() {
        let out = rewrite_subagent_prompt(&assembled_prompt(), &opts(true, false, false));
        assert!(!out.contains("<name>deploy</name>"));
        assert!(!out.contains(SKILLS_OPEN));
        assert!(!out.contains(SKILLS_CLOSE));
        assert!(
            out.contains("NEVER commit to main."),
            "project context was inherited"
        );
        assert!(out.contains("Current date: 2026-08-07"));
    }

    #[test]
    fn inheriting_everything_still_prefixes_the_child_boundary() {
        let out = rewrite_subagent_prompt(&assembled_prompt(), &opts(true, true, false));
        assert!(out.starts_with(CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS));
        assert!(out.contains("NEVER commit to main."));
        assert!(out.contains("<name>deploy</name>"));
    }

    /// A fanout-authorized child gets the fanout boundary, never the "do not run subagents" one —
    /// the grant and the prompt must not contradict each other.
    #[test]
    fn a_fanout_child_gets_the_fanout_boundary_only() {
        let out = rewrite_subagent_prompt("BODY", &opts(true, true, true));
        assert!(out.starts_with(CHILD_FANOUT_BOUNDARY_INSTRUCTIONS));
        assert!(!out.contains("Do not propose or run subagents."));
        assert!(out.ends_with("\n\nBODY"));
    }

    /// Re-running the rewrite must not stack boundary blocks, and the SECOND run's flags win.
    #[test]
    fn the_rewrite_is_idempotent_and_the_latest_flags_win() {
        let once = rewrite_subagent_prompt("BODY", &opts(true, true, true));
        let twice = rewrite_subagent_prompt(&once, &opts(true, true, false));
        assert!(twice.starts_with(CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS));
        assert!(
            !twice.contains(CHILD_FANOUT_BOUNDARY_INSTRUCTIONS),
            "a stale fanout grant must not survive a re-run: {twice}"
        );
        assert_eq!(
            twice
                .matches("You are a child subagent, not the parent orchestrator.")
                .count(),
            1
        );
        assert!(twice.ends_with("\n\nBODY"));
    }

    #[test]
    fn a_declared_schema_appends_the_structured_output_instruction_under_the_boundary() {
        let out = rewrite_subagent_prompt(
            "BODY",
            &PromptRewriteOptions {
                structured_output: true,
                ..opts(true, true, false)
            },
        );
        assert!(out.starts_with(CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS));
        assert!(out.contains(STRUCTURED_OUTPUT_INSTRUCTION));
        let without = rewrite_subagent_prompt("BODY", &opts(true, true, false));
        assert!(!without.contains(STRUCTURED_OUTPUT_INSTRUCTION));
    }

    /// A prompt with neither section is returned as-is by the strips (only the boundary is added).
    /// A prompt shaped like the real assembler's output when the subagents extension's
    /// `resources_discover` contribution HAS registered (`extension.rs`): the `pi-subagents`
    /// operational skill sits alongside a normal project skill.
    fn assembled_prompt_with_orchestration_skill() -> String {
        [
            "You are a coding assistant operating inside cyrup.",
            "",
            SKILLS_OPEN,
            "<available_skills>",
            "  <skill>",
            "    <name>deploy</name>",
            "    <description>Ship a release</description>",
            "    <location>/repo/.cyrup/skills/deploy/SKILL.md</location>",
            "  </skill>",
            "  <skill>",
            "    <name>pi-subagents</name>",
            "    <description>Orchestrate subagents</description>",
            "    <location>/pkg/resources/skills/pi-subagents/SKILL.md</location>",
            "  </skill>",
            "</available_skills>",
            "",
            "Current date: 2026-08-09",
        ]
        .join("\n")
    }

    /// The orchestration skill is removed and EVERY other skill survives, byte for byte.
    #[test]
    fn the_orchestration_skill_entry_is_removed_and_others_survive() {
        let out = strip_subagent_orchestration_skill(&assembled_prompt_with_orchestration_skill());
        assert!(
            !out.contains("pi-subagents"),
            "the orchestration entry must be gone: {out}"
        );
        assert!(!out.contains("Orchestrate subagents"));
        assert!(
            out.contains("<name>deploy</name>"),
            "the unrelated skill survives: {out}"
        );
        assert!(out.contains("Ship a release"));
        assert!(out.contains("<available_skills>") && out.contains("</available_skills>"));
        assert_eq!(
            out.matches("<skill>").count(),
            1,
            "exactly one entry left: {out}"
        );
        assert!(out.contains("Current date: 2026-08-09"));
    }

    /// pi calls it UNCONDITIONALLY (`:108`), outside both inherit guards — so a child that inherits
    /// every OTHER skill still loses this one.
    #[test]
    fn a_child_that_inherits_skills_still_loses_the_orchestration_skill() {
        let out = rewrite_subagent_prompt(
            &assembled_prompt_with_orchestration_skill(),
            &opts(true, true, false),
        );
        assert!(
            out.contains("<name>deploy</name>"),
            "skills were inherited: {out}"
        );
        assert!(
            !out.contains("pi-subagents"),
            "the parent's orchestration skill must never survive into a child: {out}"
        );
    }

    /// A prompt with no orchestration entry is returned unchanged — no stray whitespace edits.
    #[test]
    fn stripping_the_orchestration_skill_is_a_no_op_when_absent() {
        let input = assembled_prompt();
        assert_eq!(strip_subagent_orchestration_skill(&input), input);
        assert_eq!(
            strip_subagent_orchestration_skill("no skills here"),
            "no skills here"
        );
    }

    /// Only an exact `<name>pi-subagents</name>` matches — a skill that merely MENTIONS the name in
    /// its description is left alone (pi's pattern anchors on the `<name>` element, `:47`).
    #[test]
    fn a_skill_that_only_mentions_the_name_is_not_removed() {
        let input = [
            "<available_skills>",
            "  <skill>",
            "    <name>delegation-guide</name>",
            "    <description>How to use pi-subagents effectively</description>",
            "  </skill>",
            "</available_skills>",
        ]
        .join("\n");
        assert_eq!(strip_subagent_orchestration_skill(&input), input);
    }

    #[test]
    fn stripping_a_missing_section_is_a_no_op() {
        assert_eq!(
            strip_project_context("no sections here"),
            "no sections here"
        );
        assert_eq!(
            strip_inherited_skills("no sections here"),
            "no sections here"
        );
    }

    fn custom(kind: &str) -> AgentMessage {
        AgentMessage::Custom {
            kind: kind.to_string(),
            payload: serde_json::json!({}),
            details: None,
            display: true,
            timestamp: None,
        }
    }

    fn tool_result(tool_name: &str) -> AgentMessage {
        AgentMessage::ToolResult(cyrup_agent::ToolResultMessage {
            tool_call_id: ToolCallId::from("tc-1"),
            tool_name: tool_name.to_string(),
            content: vec![Content::text("done")],
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            is_error: false,
            timestamp: 0,
        })
    }

    fn assistant(blocks: Vec<Content>) -> AgentMessage {
        let mut msg = cyrup_core::AssistantMessage::errored(
            "faux".into(),
            "m",
            Some("faux".into()),
            cyrup_core::StopReason::Stop,
            "x",
        );
        msg.content = blocks;
        AgentMessage::Assistant(std::sync::Arc::new(msg))
    }

    fn tool_call_block(name: &str) -> Content {
        Content::ToolCall(cyrup_core::ToolCall {
            id: ToolCallId::from("tc-1"),
            name: name.to_string(),
            arguments: serde_json::Map::new().into(),
            thought_signature: None,
        })
    }

    /// PERF-002 flipped the stripper to `&[Arc<AgentMessage>]`; these fixtures build owned
    /// messages, so wrap them at the call rather than restating each one.
    fn arcs(msgs: Vec<AgentMessage>) -> Vec<Arc<AgentMessage>> {
        msgs.into_iter().map(Arc::new).collect()
    }

    #[test]
    fn every_parent_only_custom_type_is_dropped_from_a_childs_context() {
        for kind in PARENT_ONLY_CUSTOM_MESSAGE_TYPES {
            let messages = vec![AgentMessage::user_text("task"), custom(kind)];
            let out = strip_parent_only_subagent_messages(&arcs(messages), false)
                .unwrap_or_else(|| panic!("{kind} must be stripped"));
            assert_eq!(out.len(), 1, "{kind} must be dropped");
        }
    }

    #[test]
    fn a_plain_child_loses_the_parents_subagent_calls_and_results() {
        let messages = vec![
            AgentMessage::user_text("task"),
            assistant(vec![
                Content::text("delegating"),
                tool_call_block("subagent"),
            ]),
            tool_result("subagent"),
            tool_result("bash"),
        ];
        let out = strip_parent_only_subagent_messages(&arcs(messages), false).expect("changed");
        assert_eq!(out.len(), 3, "only the subagent toolResult is dropped");
        match out.get(1).map(|m| m.as_ref()) {
            Some(AgentMessage::Assistant(a)) => {
                assert_eq!(a.content.len(), 1, "the subagent toolCall block is gone");
                assert!(matches!(a.content.first(), Some(Content::Text { .. })));
            }
            other => panic!("expected an assistant message, got {other:?}"),
        }
        assert!(
            matches!(out.get(2).map(|m| m.as_ref()), Some(AgentMessage::ToolResult(tr)) if tr.tool_name == "bash"),
            "an unrelated tool result must survive"
        );
    }

    /// An assistant message that was ONLY a `subagent` call has nothing left to say and is dropped
    /// (pi returns `undefined` for it, `:137`).
    #[test]
    fn an_assistant_message_that_was_only_a_subagent_call_is_dropped() {
        let messages = vec![assistant(vec![tool_call_block("subagent")])];
        let out = strip_parent_only_subagent_messages(&arcs(messages), false).expect("changed");
        assert!(out.is_empty());
    }

    /// A fanout-authorized child keeps its OWN delegation history — those calls are its work.
    #[test]
    fn a_fanout_child_keeps_its_own_subagent_history_but_still_loses_parent_notices() {
        let messages = vec![
            assistant(vec![tool_call_block("subagent")]),
            tool_result("subagent"),
            custom("subagent-notify"),
        ];
        let out = strip_parent_only_subagent_messages(&arcs(messages), true)
            .expect("the notice changed it");
        assert_eq!(out.len(), 2, "both subagent tool messages survive");
        assert!(
            out.iter()
                .all(|m| !matches!(m.as_ref(), AgentMessage::Custom { .. }))
        );
    }

    /// Nothing to strip must report NO change, so the dispatcher leaves the list untouched rather
    /// than treating an identical copy as a mutation.
    #[test]
    fn a_clean_context_reports_no_change() {
        let messages = vec![AgentMessage::user_text("task"), tool_result("bash")];
        assert!(strip_parent_only_subagent_messages(&arcs(messages), false).is_none());
    }
}
