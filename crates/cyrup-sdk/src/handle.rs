//! [`Session`] — the ergonomic embedder handle over [`cyrup_session_svc::AgentSession`].
//!
//! This adds **no behaviour**; it is a thin, documented, stable surface over the facade (the one
//! integration seam, func-11 R-11-023). The agent loop, persistence, tools, and extensions all live
//! in `cyrup-session-svc` and below — `Session` only forwards calls and offers a couple of
//! collect-to-completion conveniences so a typical embedder needs no stream plumbing of its own.

use std::sync::Arc;

use cyrup_core::{EventStream, ModelRef, SessionId};
use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionServices, PromptAccepted, UserInput,
};
use futures::StreamExt;

use crate::error::SdkResult;

/// An in-process agent session an embedder drives programmatically.
///
/// Construct one with [`crate::Cyrup::builder`] then [`crate::CyrupBuilder::build_session`]. Each
/// method maps to the underlying [`AgentSession`]; the run-to-completion helpers
/// ([`Session::run`]/[`Session::run_collecting`]) are the easiest entry points for one-shot use.
///
/// `Session` is cheap to hold and every method takes `&self`, so it can be shared behind an `Arc`
/// across tasks.
///
/// # Examples
/// ```no_run
/// # async fn demo(session: cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
/// // Run a prompt to completion and read the final assistant text.
/// let answer = session.run("summarise the repo").await?;
/// println!("{answer}");
/// # Ok(()) }
/// ```
pub struct Session {
    inner: Arc<AgentSession>,
}

impl Session {
    /// Wrap a built [`AgentSession`]. Called by [`crate::CyrupBuilder::build_session`]. Binds the
    /// session's self-handle (via `into_shared`) so the post-run execution loop — auto-retry,
    /// post-run auto-compaction, queued continuations — actually fires from a completed turn.
    pub(crate) fn new(inner: AgentSession) -> Self {
        Self { inner: inner.into_shared() }
    }

    /// Borrow the underlying facade for any method not surfaced here (escape hatch).
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// let id = session.agent_session().session_id().clone();
    /// let _ = id;
    /// # }
    /// ```
    pub fn agent_session(&self) -> &AgentSession {
        &self.inner
    }

    /// The shared session `Arc` (clone-cheap; share across tasks).
    pub fn agent_session_arc(&self) -> Arc<AgentSession> {
        self.inner.clone()
    }

    /// Consume the handle and return the shared [`AgentSession`] (the post-run driver + subscriber
    /// hold weak references to this same `Arc`, so the seam is returned as the `Arc` rather than moved
    /// out of it).
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: cyrup_sdk::Session) {
    /// let facade = session.into_inner();
    /// let _ = facade;
    /// # }
    /// ```
    pub fn into_inner(self) -> Arc<AgentSession> {
        self.inner
    }

    /// Tear the session down: settle any in-flight run, announce `session_shutdown{reason:"quit"}`
    /// to every loaded extension, and cancel the long-lived session token so the extension
    /// subscriber unwinds.
    ///
    /// SEAM-026/SEAM-002 — the mirror image of the `session_start` [`crate::CyrupBuilder::build_session`]
    /// emits, and pi's `AgentSessionRuntime.dispose()` (agent-session-runtime.ts:398-404, which
    /// emits `session_shutdown{reason:"quit"}` and then disposes the session). Before this the SDK
    /// had NO teardown call at all: an embedder dropping a `Session` left every extension without a
    /// shutdown hook, so audit logs never flushed, intercom identities were never deregistered and
    /// spooled permission asks were never released.
    ///
    /// This is `async` and consuming rather than a `Drop` impl on purpose: `session_shutdown` is
    /// dispatched to extensions (possibly across a wasm boundary) and awaited, which `Drop` cannot
    /// do. Dropping a `Session` without calling this is therefore still silent — call it.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: cyrup_sdk::Session) {
    /// session.close().await;
    /// # }
    /// ```
    pub async fn close(self) {
        self.inner.dispose("quit").await;
    }

    // ----------------------------------------------------------------- run helpers ----

    /// Submit a prompt, drive the run to completion, and return the final assistant text.
    ///
    /// Drains the run's event stream to its terminal `agent_end`, then reads the last assistant
    /// message text on the current branch (empty string if the model produced none).
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
    /// let text = session.run("what is 2 + 2?").await?;
    /// assert!(!text.is_empty());
    /// # Ok(()) }
    /// ```
    pub async fn run(&self, input: impl Into<UserInput>) -> SdkResult<String> {
        let mut stream = self.inner.prompt(input).await?;
        // Drain to the terminal `agent_end`; the stream ends when the run settles.
        while stream.next().await.is_some() {}
        Ok(self.inner.last_assistant_text().await.unwrap_or_default())
    }

    /// Submit a prompt and return every [`AgentSessionEvent`] of the run, in order, plus the final
    /// assistant text.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
    /// let (events, text) = session.run_collecting("hello").await?;
    /// assert_eq!(events.first().map(|e| e.kind()), Some("agent_start"));
    /// assert_eq!(events.last().map(|e| e.kind()), Some("agent_end"));
    /// let _ = text;
    /// # Ok(()) }
    /// ```
    pub async fn run_collecting(
        &self,
        input: impl Into<UserInput>,
    ) -> SdkResult<(Vec<AgentSessionEvent>, Option<String>)> {
        let stream = self.inner.prompt(input).await?;
        let events: Vec<AgentSessionEvent> = stream.collect().await;
        let text = self.inner.last_assistant_text().await;
        Ok((events, text))
    }

    // ----------------------------------------------------------------- prompting ----

    /// Submit a prompt and observe the run as a stream of [`AgentSessionEvent`].
    ///
    /// The stream terminates after the run's `agent_end`. Fails only if the prompt could not be
    /// *accepted* (e.g. a run is already streaming — use [`Session::steer`]/[`Session::follow_up`]).
    ///
    /// # Examples
    /// ```no_run
    /// # use futures::StreamExt;
    /// # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
    /// let mut stream = session.prompt("hi").await?;
    /// while let Some(event) = stream.next().await {
    ///     println!("{}", event.kind());
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn prompt(
        &self,
        input: impl Into<UserInput>,
    ) -> SdkResult<EventStream<AgentSessionEvent>> {
        Ok(self.inner.prompt(input).await?)
    }

    /// Submit a prompt, resolving only to the preflight acceptance (the run is observed via
    /// [`Session::subscribe`]). For embedders that manage their own persistent subscription.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
    /// let _events = session.subscribe();
    /// let _accepted = session.prompt_accepted("go").await?;
    /// # Ok(()) }
    /// ```
    pub async fn prompt_accepted(
        &self,
        input: impl Into<UserInput>,
    ) -> SdkResult<PromptAccepted> {
        Ok(self.inner.prompt_accepted(input).await?)
    }

    /// A long-lived event subscription that lives until the returned stream is dropped.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// let _all_events = session.subscribe();
    /// # }
    /// ```
    pub fn subscribe(&self) -> EventStream<AgentSessionEvent> {
        self.inner.subscribe()
    }

    /// Enqueue a steering message (delivered after the current tool batch, func-02 §9).
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
    /// session.steer("actually, focus on the tests").await?;
    /// # Ok(()) }
    /// ```
    pub async fn steer(&self, input: impl Into<UserInput>) -> SdkResult<PromptAccepted> {
        Ok(self.inner.steer(input).await?)
    }

    /// Enqueue a follow-up message (delivered after the agent goes idle, func-02 §9).
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
    /// session.follow_up("now write a changelog entry").await?;
    /// # Ok(()) }
    /// ```
    pub async fn follow_up(&self, input: impl Into<UserInput>) -> SdkResult<PromptAccepted> {
        Ok(self.inner.follow_up(input).await?)
    }

    /// Interrupt the active run and its auto-retry backoff (idempotent; R-11-018). Returns as soon
    /// as the cancellation is signalled — use [`Session::abort_and_settle`] to await the run
    /// actually stopping.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// session.abort();
    /// # }
    /// ```
    pub fn abort(&self) {
        self.inner.abort();
    }

    /// Interrupt the active run **and await its settlement** — pi's `await session.abort()`
    /// (`abortRetry(); agent.abort(); await waitForIdle()`, agent-session.ts:1542-1546). Use this
    /// wherever "aborted" must mean "stopped", e.g. before inspecting the transcript or compacting.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// session.abort_and_settle().await;
    /// # }
    /// ```
    pub async fn abort_and_settle(&self) {
        self.inner.abort_and_settle().await;
    }

    /// Await full settlement of the in-flight run (`agent_end`).
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// session.wait_for_idle().await;
    /// # }
    /// ```
    pub async fn wait_for_idle(&self) {
        self.inner.wait_for_idle().await;
    }

    // ----------------------------------------------------------------- compaction ----

    /// Compact the current branch; returns the produced [`cyrup_session_svc::CompactionResult`].
    ///
    /// A refusal is an `Err` carrying the reason (`Nothing to compact (session too small)` /
    /// `Already compacted` / `Compaction cancelled`), 1:1 with Pi's `compact`, which is typed
    /// `Promise<CompactionResult>` and `throw`s (agent-session.ts:1801-1825) — never a success value
    /// an embedder has to reverse-engineer.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
    /// let compacted = session.compact(None).await?;
    /// let _ = compacted.summary;
    /// # Ok(()) }
    /// ```
    pub async fn compact(
        &self,
        custom_instructions: Option<String>,
    ) -> SdkResult<cyrup_session_svc::CompactionResult> {
        Ok(self.inner.compact(custom_instructions).await?)
    }

    // --------------------------------------------------------------- fork / branch ----

    /// Fork the current persisted session into a new file under the same cwd; returns the new id.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
    /// let new_id = session.fork().await?;
    /// let _ = new_id;
    /// # Ok(()) }
    /// ```
    pub async fn fork(&self) -> SdkResult<SessionId> {
        Ok(self.inner.fork().await?)
    }

    /// Navigate the session leaf to `entry` (no file mutation).
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session, entry: cyrup_core::EntryId) -> cyrup_sdk::SdkResult<()> {
    /// session.branch(entry).await?;
    /// # Ok(()) }
    /// ```
    pub async fn branch(&self, entry: cyrup_core::EntryId) -> SdkResult<()> {
        Ok(self.inner.branch(entry).await?)
    }

    // --------------------------------------------------------------- model control ----

    /// Switch the active model by pattern (`provider/id[:level]`); returns the resolved address.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
    /// let model = session.set_model("anthropic/claude-opus-4").await?;
    /// let _ = model;
    /// # Ok(()) }
    /// ```
    pub async fn set_model(&self, pattern: &str) -> SdkResult<ModelRef> {
        Ok(self.inner.set_model(pattern).await?)
    }

    /// The current model address.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// let model = session.model();
    /// println!("{}/{}", model.provider, model.model);
    /// # }
    /// ```
    pub fn model(&self) -> ModelRef {
        self.inner.model()
    }

    // ----------------------------------------------------------------- read access ----

    /// Whether a run is currently streaming.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// if session.is_streaming().await {
    ///     session.abort();
    /// }
    /// # }
    /// ```
    pub async fn is_streaming(&self) -> bool {
        self.inner.is_streaming().await
    }

    /// This session's id.
    ///
    /// # Examples
    /// ```no_run
    /// # fn demo(session: &cyrup_sdk::Session) {
    /// let id = session.session_id();
    /// let _ = id;
    /// # }
    /// ```
    pub fn session_id(&self) -> &SessionId {
        self.inner.session_id()
    }

    /// The on-disk session file, if this session is persisted.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// if let Some(path) = session.session_file().await {
    ///     println!("{}", path.display());
    /// }
    /// # }
    /// ```
    pub async fn session_file(&self) -> Option<std::path::PathBuf> {
        self.inner.session_file().await
    }

    /// The persisted transcript messages on the current branch.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// let messages = session.messages().await;
    /// println!("{} messages", messages.len());
    /// # }
    /// ```
    pub async fn messages(&self) -> Vec<cyrup_core::Message> {
        self.inner.messages().await
    }

    /// The agent's current in-memory transcript (includes any streaming partial).
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// let live = session.agent_messages().await;
    /// let _ = live;
    /// # }
    /// ```
    pub async fn agent_messages(&self) -> Vec<cyrup_agent::AgentMessage> {
        self.inner.agent_messages().await
    }

    /// The most recent assistant message text on the current branch.
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo(session: &cyrup_sdk::Session) {
    /// let last = session.last_assistant_text().await;
    /// let _ = last;
    /// # }
    /// ```
    pub async fn last_assistant_text(&self) -> Option<String> {
        self.inner.last_assistant_text().await
    }

    /// The assembled system prompt for this session.
    ///
    /// # Examples
    /// ```no_run
    /// # fn demo(session: &cyrup_sdk::Session) {
    /// let prompt = session.system_prompt();
    /// assert!(!prompt.is_empty());
    /// # }
    /// ```
    pub fn system_prompt(&self) -> &str {
        self.inner.system_prompt()
    }

    /// The cwd-bound services this session wired (settings/auth/resources/ext host/model/prompt).
    ///
    /// # Examples
    /// ```no_run
    /// # fn demo(session: &cyrup_sdk::Session) {
    /// let services = session.services();
    /// println!("trusted: {}", services.project_trusted);
    /// # }
    /// ```
    pub fn services(&self) -> &AgentSessionServices {
        self.inner.services()
    }
}
