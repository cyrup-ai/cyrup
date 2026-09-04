//! The immediate-bash seam — out-of-loop `!`/`!!` and RPC `bash` execution.
//!
//! Pi `agent-session.ts:2582-2684`. Runs a command outside the agent loop, streams its combined
//! output to the caller's sink, and records the result into the transcript (deferring it while a
//! run streams). Concurrent calls each keep their own cancel handle.

use std::sync::atomic::Ordering;

use cyrup_agent::{AgentMessage, AppRole};
use cyrup_core::CancelToken;
use cyrup_ext::{HostEvent, Reduced};
use cyrup_tools::ShellConfig;

use crate::bash::{BashOptions, BashResult, bash_message_payload, run_bash};
use crate::error::SessionServiceError;
use crate::event::AgentSessionEvent;

use super::{AgentSession, now_ms};

/// Removes this call's entry from [`AgentSession::bash_cancels`] on drop — cyrup's stand-in for pi's
/// `finally { this._bashAbortControllers.delete(abortController); }` (agent-session.ts:2794-2796
/// @v0.83.0).
///
/// A guard rather than a removal written at each exit, because pi's `finally` covers exits Rust
/// reaches differently: the early `?` on `Custom shell path not found`, the `?` on the backend
/// failure pi lets propagate straight *through* its `finally` uncaught, and — with no JS counterpart
/// at all — an `execute_bash` future DROPPED at an `.await` by a caller that went away, which a JS
/// `async` function cannot do because it always settles. Without the guard that last case leaks a
/// dead handle into the set and makes `is_bash_running()` answer true forever.
pub(super) struct BashCancelGuard<'a> {
    session: &'a AgentSession,
    id: u64,
}

impl Drop for BashCancelGuard<'_> {
    fn drop(&mut self) {
        AgentSession::lock(&self.session.bash_cancels).retain(|(id, _)| *id != self.id);
    }
}

impl AgentSession {
    /// Execute a bash command out-of-band and record its result (Pi `executeBash`,
    /// agent-session.ts:2588). Streams combined output to `on_chunk`; the result is recorded into the
    /// transcript (or deferred while a run streams).
    ///
    /// Fires NO extension event of its own — Pi's `executeBash` (agent-session.ts:2582-2684) has zero
    /// `emitUserBash` emission even at HEAD; in Pi the emission lives at the two front-end CALLERS,
    /// which each emit `user_bash` for themselves and only then call into this executor:
    /// `interactive-mode.ts:6010-6060`'s `handleBashCommand` (the `!`/`!!`-prefix handler) and
    /// `rpc-mode.ts:558-579`'s `case "bash"` (given its emission by pi `5d548ae9`, 2026-07-28,
    /// "fix: rpc bash no longer bypass user_bash", #7214). cyrup shares one wrapper across both:
    /// [`Self::execute_bash_with_user_event`] — that is what `crates/cyrup-modes/src/rpc.rs`'s
    /// `SessionCommand::Bash` arm calls. Call this bare method only when the caller is NOT a
    /// user-initiated bash front-end (it is also the fall-through of that wrapper).
    ///
    /// A genuine backend failure is returned as `Err` and NEVER recorded into history — Pi's
    /// `executeBash` only calls `recordBashResult` on the success path inside its `try` block
    /// (`agent-session.ts:2628-2643`); a rejection from `executeBashWithOperations` propagates
    /// straight through the `finally` (which only deletes this call's handle from
    /// `_bashAbortControllers`, agent-session.ts:2794-2796) uncaught, all the way to the RPC
    /// dispatcher's `catch` (`rpc-mode.ts:756-772`).
    pub async fn execute_bash(
        &self,
        command: &str,
        options: BashOptions,
        on_chunk: crate::bash::BashChunkSink,
    ) -> Result<BashResult, SessionServiceError> {
        // Pi `const abortController = new AbortController(); this._bashAbortControllers.add(...)`
        // (agent-session.ts:2770-2771 @v0.83.0) — one handle PER CALL, added to the set, removed in
        // the `finally` (here: `_bash_guard`'s drop). Concurrent calls each keep their own.
        let cancel = self.session_cancel.child_token();
        let bash_cancel_id = self.next_bash_cancel_id();
        Self::lock(&self.bash_cancels).push((bash_cancel_id, cancel.clone()));
        let _bash_guard = BashCancelGuard {
            session: self,
            id: bash_cancel_id,
        };
        let cwd = self.services.cwd.clone();
        // Managed bin dir (Pi `getBinDir()`, `config.ts:549`: `join(getAgentDir(), "bin")`), matching
        // `cyrup_config::ConfigDirs::bin_dir()`'s layout — see `run_bash`'s doc comment.
        let bin_dir = self.services.agent_dir.join("bin");
        // Apply the `shellCommandPrefix` setting (Pi `executeBash`, agent-session.ts:2624-2627):
        // prepend it before the command, joined by a newline — the same prefix application the
        // agent-loop `bash` tool already performs (`cyrup-tools/src/tools/bash.rs:99-102`). The
        // ORIGINAL `command` (not this resolved one) is still what gets recorded into history below,
        // matching Pi's `recordBashResult(command, result, options)` (agent-session.ts:2628).
        let resolved_command = match &self.shell_command_prefix {
            Some(prefix) => format!("{prefix}\n{command}"),
            None => command.to_string(),
        };
        // Resolve the shell fresh on THIS call (Pi's `createLocalBashOperations({ shellPath })`
        // resolves `getShellConfig(shellPath)` inside `exec` on every `executeBash` invocation —
        // bash.ts:91,159 — never baked in once at session build time). BOTH of Pi's errors surface
        // here, identically to the agent-loop `bash` tool: `Custom shell path not found: …` when
        // `shellPath` is set and missing (shell.ts:73), and the three-option `No bash shell found.
        // Options: …` recipe with its `Searched Git Bash in:` list when it is unset and the host
        // has no bash (shell.ts:100-106). `_bash_guard` performs Pi's `finally` removal on this
        // early-return path too.
        let shell = ShellConfig::resolve(self.shell_path.as_deref())?;
        // Pi wraps the caller's `onChunk` and emits `bash_execution_update` for EVERY delta,
        // whether or not a sink was supplied (agent-session.ts:2784-2787):
        //   onChunk: (delta) => { onChunk?.(delta); this._emit({type:"bash_execution_update", …}) }
        // so a front-end that only observes events still renders live output. The sync callback
        // posts to a queue drained by `spawn_event_pump` (see its doc for why).
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::unbounded_channel();
        let bash_id = options.id.clone();
        let mut caller_sink = on_chunk;
        let sink: crate::bash::BashChunkSink = Some(Box::new(move |delta: &str| {
            if let Some(cb) = caller_sink.as_mut() {
                cb(delta);
            }
            let _ = chunk_tx.send(AgentSessionEvent::BashExecutionUpdate {
                id: bash_id.clone(),
                delta: delta.to_string(),
            });
        }));
        let chunk_pump = self.spawn_event_pump(chunk_rx);
        // Pi `options?.operations ?? createLocalBashOperations({ shellPath })`
        // (agent-session.ts:2782): a caller-supplied backend replaces the local one for THIS call
        // only. `run_bash` takes the `??` whole — `None` is the local branch it always took.
        let outcome = run_bash(
            &self.proc,
            &shell,
            options.operations.as_deref(),
            cwd,
            resolved_command,
            Some(bin_dir.as_path()),
            cancel,
            sink,
        )
        .await;
        // `run_bash` consumed the sink, so its `chunk_tx` is already dropped: awaiting the pump
        // flushes every delta before the caller sees the result.
        let _ = chunk_pump.await;
        // No explicit removal here — `_bash_guard` is pi's `finally` and fires on BOTH the `?` below
        // and the `Ok` return, and only for THIS call's handle. Clearing the whole slot here is what
        // used to make `is_bash_running()` lie while a concurrent command was still running.
        let result = outcome?;
        self.record_bash_result(command, &result, options).await;
        Ok(result)
    }

    /// Execute a **user-initiated** bash command: the entry point every user-facing bash front-end
    /// must call. Fires the `user_bash` extension event FIRST with the live `{command,
    /// excludeFromContext, cwd}` (Pi `UserBashEvent`, `extensions/types.ts:813-821`); a handler that
    /// returns a full `result` override (`UserBashEventResult.result`,
    /// `extensions/types.ts:1078-1083`) short-circuits local execution entirely and its result is
    /// still recorded through [`Self::record_bash_result`]; otherwise this falls through to the bare
    /// [`Self::execute_bash`] for normal execution.
    ///
    /// Pi emits at both front-ends rather than inside `executeBash`: the interactive `!`/`!!`-prefix
    /// handler (`interactive-mode.ts:6010-6060`, `handleBashCommand`) and the JSON-RPC `bash`
    /// command (`rpc-mode.ts:558-579`, `case "bash"` — emission added by pi `5d548ae9`, 2026-07-28,
    /// "fix: rpc bash no longer bypass user_bash", #7214, so an extension observing user bash no
    /// longer misses RPC-issued commands). Both cyrup front-ends therefore share this one wrapper.
    ///
    /// Pi's sibling `operations` remote-exec override — the other half of `UserBashEventResult` —
    /// is threaded through [`BashOptions::operations`] and honored by [`Self::execute_bash`]
    /// (`options?.operations ?? createLocalBashOperations({ shellPath })`,
    /// `agent-session.ts:2782`). **What this wrapper still cannot do is FILL it from the event
    /// result, and that is DRIFT-004 / SEAM-015's last remaining half.** Upstream's
    /// `rpc-mode.ts:576` can write `operations: eventResult?.operations` because a JS handler
    /// returns a live object; cyrup's extension I/O is serde values (ADR-0002), so the payload
    /// [`Self::emit_user_bash_event`] receives can carry the `operations` KEY but never a callable
    /// behind it. Closing it needs the `register-bash-operations` import + keyed
    /// `bash-operations-exec` export round-trip designed in full in `crates/cyrup-ext/src/lib.rs`'s
    /// CYRUP-DELTA register — a `crates/cyrup-ext` + `crates/cyrup-ext-sdk` change with a
    /// `HOST_WORLD` minor bump, not a change here. Once it exists this wrapper sets one field.
    pub async fn execute_bash_with_user_event(
        &self,
        command: &str,
        options: BashOptions,
        on_chunk: crate::bash::BashChunkSink,
    ) -> Result<BashResult, SessionServiceError> {
        if let Some(result) = self
            .emit_user_bash_event(command, options.exclude_from_context)
            .await
        {
            self.record_bash_result(command, &result, options).await;
            return Ok(result);
        }
        self.execute_bash(command, options, on_chunk).await
    }

    /// Emit the `user_bash` extension event and, if a handler fully serviced the command (Pi
    /// `UserBashEventResult.result`, `extensions/types.ts:1078-1083`), return its [`BashResult`]
    /// override so the caller skips local execution. Returns `None` when nobody subscribed or no
    /// result override was supplied. Carries the live `command`, the `exclude_from_context` flag
    /// (the interactive `!!` prefix, or the RPC command's `excludeFromContext ?? false`,
    /// `rpc-mode.ts:562`), and the session cwd (Pi `UserBashEvent`, `extensions/types.ts:813-821`).
    ///
    /// Matches Pi's `emitUserBash` (`extensions/runner.ts:955-981`) dispatch semantics: the FIRST
    /// truthy handler result wins and short-circuits the remaining handlers, and a handler that
    /// throws is caught and reported rather than being fatal — `dispatch_block_mutate` returning
    /// `Reduced::Handled` is cyrup's equivalent of the former, and the dispatcher's per-extension
    /// error isolation of the latter.
    async fn emit_user_bash_event(
        &self,
        command: &str,
        exclude_from_context: bool,
    ) -> Option<BashResult> {
        if self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::UserBash)
        {
            return None;
        }
        let cancel = self.session_cancel.child_token();
        let event = HostEvent::UserBash {
            command: command.to_string(),
            exclude_from_context,
            cwd: self.services.cwd.display().to_string(),
        };
        let reduced = self
            .services
            .ext_host
            .dispatcher()
            .dispatch_block_mutate(event, &cancel)
            .await;
        // A handler that returned a `UserBashEventResult.result` (Pi types.ts:1043-1048) fully
        // serviced the command; deserialize the override `BashResult`. Other outcomes fall through
        // to normal execution.
        if let Reduced::Handled(handled) = reduced {
            return handled
                .0
                .get("result")
                .cloned()
                .and_then(|r| serde_json::from_value::<BashResult>(r).ok());
        }
        None
    }

    /// Record a bash result into the transcript + session (Pi `recordBashResult`,
    /// agent-session.ts:2628). While a run streams, the message is deferred to avoid breaking
    /// tool_use/tool_result ordering and flushed after the turn.
    pub async fn record_bash_result(
        &self,
        command: &str,
        result: &BashResult,
        options: BashOptions,
    ) {
        let payload = bash_message_payload(command, result, options.exclude_from_context);
        // The transcript message is the full pi wire object — `role` and `timestamp` included —
        // which is exactly what a compaction re-seed produces for the same execution via
        // `raw_message_to_agent`, so the live and resumed messages are one variant, one shape.
        // The persisted entry (`append_bash_message` below) keeps the BARE payload: the session
        // store supplies `custom_type` and `timestamp` itself, and its bytes must not change.
        let serde_json::Value::Object(mut wire) = payload.clone() else {
            // Unreachable: `bash_message_payload` always builds an object.
            return;
        };
        wire.insert(
            "role".to_string(),
            serde_json::Value::from(AppRole::BashExecution.as_str()),
        );
        wire.insert("timestamp".to_string(), serde_json::Value::from(now_ms()));
        let msg = AgentMessage::App {
            role: AppRole::BashExecution,
            payload: wire,
        };
        // AGENT-030 — pi defers on `this.isStreaming`, the session latch `_isAgentRunActive`
        // (agent-session.ts:900-901, :3007: "If agent is streaming, defer adding to avoid breaking
        // tool_use/tool_result ordering"), so a result landing in the post-`agent_end` gap waits for
        // the WHOLE loop's `flush_pending_bash_messages` (`run.rs`), pi's `finally`.
        if self.is_run_active() {
            Self::lock(&self.pending_bash).push(msg);
            return;
        }
        self.append_bash_message(msg, &payload).await;
    }

    /// Mint the next identity handle for [`Self::bash_cancels`] (pi gets identity for free from the
    /// `AbortController` object it puts in the set).
    fn next_bash_cancel_id(&self) -> u64 {
        self.next_bash_cancel_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Cancel EVERY running bash command (Pi `abortBash`, agent-session.ts:2832-2836 @v0.83.0:
    /// `for (const abortController of [...this._bashAbortControllers]) abortController.abort();`).
    ///
    /// The snapshot is pi's spread copy, and it is load-bearing here for a reason pi does not have:
    /// `CancelToken::cancel` runs registered callbacks synchronously, one of which could re-enter
    /// this session, so the lock is released before any token is fired.
    pub fn abort_bash(&self) {
        let snapshot: Vec<CancelToken> = Self::lock(&self.bash_cancels)
            .iter()
            .map(|(_, c)| c.clone())
            .collect();
        for c in snapshot {
            c.cancel();
        }
    }

    /// Whether ANY bash command is running (Pi `isBashRunning`, agent-session.ts:2839-2841
    /// @v0.83.0: `return this._bashAbortControllers.size > 0;`).
    pub fn is_bash_running(&self) -> bool {
        !Self::lock(&self.bash_cancels).is_empty()
    }

    /// Whether deferred bash messages await flush (Pi `hasPendingBashMessages`, agent-session.ts:2670).
    pub fn has_pending_bash_messages(&self) -> bool {
        !Self::lock(&self.pending_bash).is_empty()
    }

    /// Flush deferred bash messages to the transcript + session (Pi `_flushPendingBashMessages`,
    /// agent-session.ts:2675). Called before a new prompt so ordering is intact.
    pub async fn flush_pending_bash_messages(&self) {
        let pending: Vec<AgentMessage> = std::mem::take(&mut *Self::lock(&self.pending_bash));
        for msg in pending {
            if let AgentMessage::App { payload, .. } = &msg {
                let mut bare = payload.clone();
                bare.remove("role");
                bare.remove("timestamp");
                let bare = serde_json::Value::Object(bare);
                self.append_bash_message(msg, &bare).await;
            }
        }
    }

    /// Append a bash message to the agent transcript + persist it durably.
    async fn append_bash_message(&self, msg: AgentMessage, payload: &serde_json::Value) {
        // One locked edit. Reached only after the run has settled (`flush_pending_bash_messages`)
        // or when not streaming (`record_bash_result`), so `RunActive` cannot occur.
        let _ = self.agent.edit_transcript(|m| m.push(msg));
        let _ = self.manager.lock().await.append_custom_message(
            "bashExecution",
            payload.clone(),
            true,
            None,
        );
    }
}
