//! The plain read accessors over the session's live state.
//!
//! Pi's getter surface (agent-session.ts:866-877 and friends): active model, session identity and
//! header, the system-prompt trio (`base` / `override` / `effective`), the built context and
//! messages, trust + settings seams, and the facade handles onto resources, the extension host and
//! the model catalog. No method here mutates anything.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use cyrup_core::{AssistantMessage, EntryId, Message, ModelRef, SessionId};
use cyrup_session::context::SessionContext;
use cyrup_session::header::SessionHeader;

use crate::error::SessionServiceError;
use crate::services::AgentSessionServices;

use super::AgentSession;

// Doc-only — see `types.rs`. `model_catalog`'s doc names the swappable provider; the original
// `session.rs` had it in scope via `use crate::provider_swap::ProviderSwap` (session.rs:41).
#[cfg(doc)]
use crate::provider_swap::ProviderSwap;

impl AgentSession {
    /// The current model address, or `None` when the session has no model at all.
    ///
    /// Pi `get model(): Model<any> | undefined { return this.agent.state.model; }`
    /// (agent-session.ts:865-868) — documented there as "may be undefined if not yet selected".
    /// `None` is the state a credential-less first run launches in (SEAM-075): `main.ts:852-855`
    /// exits 1 on it in every NON-interactive mode, while interactive shows the
    /// `modelFallbackMessage` banner and waits for `/login` + `/model`.
    pub fn model(&self) -> Option<ModelRef> {
        Self::lock(&self.model).clone()
    }

    /// The model-restore fallback warning, if the resumed session's saved model was unavailable
    /// (Pi `modelFallbackMessage`, sdk.ts:91).
    pub fn model_fallback_message(&self) -> Option<&str> {
        self.model_fallback_message.as_deref()
    }

    /// Whether the AGENT's run is in flight — its run latch, released the moment each individual
    /// run settles. [CYRUP-DELTA] pi's `get isStreaming()` (agent-session.ts:900-901) returns the
    /// SESSION latch `_isAgentRunActive`, which also spans the post-run driver loop; that predicate
    /// is [`Self::is_run_active`], and every routing decision pi makes on its getter uses it. This
    /// accessor keeps the narrower agent latch because cyrup has ONE session signal where pi has
    /// two: `driver_tx` is both the run-active latch and the idle latch, so it must drop AFTER
    /// `fanout.end_run()` (or a `prompt` issued from `wait_for_idle` would register a run-scoped
    /// stream the previous run's `end_run` then clears) — and SDK / embedding callers assert
    /// idleness the instant the run-scoped stream ends (`session/run.rs`).
    pub async fn is_streaming(&self) -> bool {
        self.agent.snapshot().await.is_streaming
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The session header record (Pi `sessionManager.getHeader()`, session-manager.ts:1208-1211):
    /// `{type:"session", version, id, timestamp, cwd, parentSession?}`. This is the passthrough a
    /// `--mode json` run serializes as JSONL line 1 before the event stream (Pi print-mode.ts:112-117).
    /// A live session always carries a header (unlike Pi's `getHeader` which is nominally nullable
    /// when no `session` entry exists — never the case for an opened/created manager), so this
    /// returns the header directly rather than an `Option`.
    pub async fn session_header(&self) -> SessionHeader {
        self.manager.lock().await.header().clone()
    }

    /// Claim the one-shot JSON-mode header emission for this session. Returns `true` for the FIRST
    /// caller and `false` thereafter, so that a multi-prompt `--mode json` run — whose initial
    /// submission and each follow-up are dispatched as separate `run_json` calls — writes the header
    /// line exactly once, matching Pi's single `getHeader()` write ahead of the whole message loop
    /// in `runPrintMode` (print-mode.ts:112-119).
    pub fn claim_json_header(&self) -> bool {
        !self.json_header_written.swap(true, Ordering::SeqCst)
    }

    /// The on-disk session file, if this session is persisted.
    pub async fn session_file(&self) -> Option<std::path::PathBuf> {
        self.manager.lock().await.session_file().map(Path::to_path_buf)
    }

    /// The cwd-bound services this session wired (settings/auth/resources/ext host/model/prompt).
    pub fn services(&self) -> &AgentSessionServices {
        &self.services
    }

    /// The captured extension CLI flag values threaded from the CLI (Pi `extensionFlagValues`,
    /// main.ts:634). A loaded extension consumes these via `applyExtensionFlagValues`; the read-only
    /// seam is surfaced here so the threading is observable end-to-end.
    pub fn extension_flag_values(&self) -> &[(String, crate::builder::ExtensionFlagValue)] {
        &self.services.extension_flag_values
    }

    /// The `trust.json` store path for this session (`agent_dir/trust.json`, Pi
    /// `EnvVars::trustPath`). The additive data seam the `/trust` selector writes through.
    pub fn trust_store_path(&self) -> std::path::PathBuf {
        self.services.agent_dir.join("trust.json")
    }

    /// The standard project-trust options for this session's cwd (Pi `getProjectTrustOptions`,
    /// trust-manager.ts:65; `cyrup_config::trust::trust_options`). Drives the `/trust` selector rows.
    pub fn project_trust_options(&self) -> Vec<cyrup_config::trust::TrustOption> {
        cyrup_config::trust::trust_options(&self.services.cwd, false)
    }

    /// The nearest saved trust decision for this session's cwd (Pi `findNearestTrustEntry`); `None`
    /// when no ancestor has a persisted decision. Read-only; surfaced in the `/trust` selector header.
    pub async fn saved_trust_decision(&self) -> Option<cyrup_config::trust::TrustEntry> {
        cyrup_config::trust::TrustStore::new(self.trust_store_path())
            .nearest(&self.services.cwd)
            .await
            .ok()
            .flatten()
    }

    /// Persist a project-trust decision (the `updates` of a [`cyrup_config::trust::TrustOption`]) to
    /// the `trust.json` store (Pi `/trust` `onSelect` → `setProjectTrust`, trust-manager.ts). An empty
    /// `updates` (session-only option) writes nothing. The in-memory `services().project_trusted`
    /// reflects the new session only after a `/reload`, matching Pi.
    pub async fn write_project_trust(
        &self,
        updates: &[(std::path::PathBuf, Option<cyrup_config::trust::TrustDecision>)],
    ) -> Result<(), SessionServiceError> {
        if updates.is_empty() {
            return Ok(());
        }
        cyrup_config::trust::TrustStore::new(self.trust_store_path()).set_many(updates).await?;
        Ok(())
    }

    /// Persist a settings field to the on-disk store (Pi `/settings`/`/config` selector apply →
    /// `SettingsManager.setNested`). Writes via the manager's `&self` store seam; the in-memory
    /// `effective()` view reflects it after a `/reload`, matching Pi's apply-then-reload flow. A
    /// dotted `key` (`terminal.showImages`) addresses a nested field. Project writes require trust.
    pub async fn persist_setting(
        &self,
        scope: cyrup_config::SettingsScope,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), SessionServiceError> {
        let path: Vec<&str> = key.split('.').filter(|s| !s.is_empty()).collect();
        self.services.settings.persist_nested(scope, &path, value).await?;
        Ok(())
    }

    /// The system prompt the builder assembled at session start (arch-06). Frozen — it does NOT
    /// track mid-session tool-set rebuilds; use [`Self::base_system_prompt`] for the live base or
    /// [`Self::current_system_prompt`] for the agent's in-flight value.
    pub fn system_prompt(&self) -> &str {
        &self.services.system_prompt
    }

    /// The LIVE base system prompt (Pi `this._baseSystemPrompt`, agent-session.ts:371) — the value a
    /// run falls back to when no `before_agent_start` handler replaced it. Equal to
    /// [`Self::system_prompt`] until a tool-set rebuild (`/tools` toggle, a guest `setActiveTools`,
    /// or EXT-004 late tool registration) rewrites it via [`Self::push_active_tools`].
    pub fn base_system_prompt(&self) -> String {
        Self::lock(&self.base_system_prompt).clone()
    }

    /// The `before_agent_start` replacement in force for the CURRENT run, if any (Pi
    /// `this._systemPromptOverride`, agent-session.ts:373 @v0.83.0). `None` between runs and
    /// whenever no handler replaced the prompt.
    pub fn system_prompt_override(&self) -> Option<String> {
        Self::lock(&self.system_prompt_override).clone()
    }

    /// `override ?? base` — the exact expression pi evaluates at every site that writes
    /// `agent.state.systemPrompt` (agent-session.ts:534 in the turn-boundary refresh, `:940` in
    /// `setActiveToolsByName` @v0.83.0). This is the value the agent must be running with at any
    /// moment, and the value the per-turn refresh re-pushes (DRIFT-033).
    pub fn effective_system_prompt(&self) -> String {
        // Two statements, not one chained expression: the override guard must be released before
        // `base_system_prompt()` takes the second lock.
        let over = Self::lock(&self.system_prompt_override).clone();
        over.unwrap_or_else(|| self.base_system_prompt())
    }

    /// The agent's *current* system prompt — equal to the base unless a `before_agent_start` handler
    /// replaced it for the in-flight run (Pi `agent.state.systemPrompt`, agent-session.ts:1127).
    pub async fn current_system_prompt(&self) -> String {
        self.agent.snapshot().await.system_prompt
    }

    /// The current LLM context built from the session tree (leaf→root, R-04-011).
    pub async fn context(&self) -> SessionContext {
        self.manager.lock().await.build_context()
    }

    /// The persisted transcript messages on the current branch (R-11-014 `get_messages`).
    ///
    /// This is the **LLM-flattened** view (`convertToLlm`): a compaction/branch summary, an
    /// extension `custom` message and a `!` bash execution all arrive as `user` messages carrying
    /// their wrapper prose. Anything that RENDERS the conversation wants
    /// [`raw_context_messages`](Self::raw_context_messages) instead.
    pub async fn messages(&self) -> Vec<Message> {
        self.manager.lock().await.build_context().messages
    }

    /// The current branch's context with its **roles intact** — Pi's
    /// `buildContextEntries().flatMap(sessionEntryToContextMessages)`
    /// (`session-manager.ts:441-453` + `:383-408`), the input Pi's `renderSessionEntries` replays a
    /// resumed session from (interactive-mode.ts:3506-3516).
    ///
    /// Unlike [`messages`](Self::messages) this has NOT been through `convertToLlm`, so a
    /// `compactionSummary` / `branchSummary` / `custom` / `bashExecution` still identifies itself and
    /// a front-end can route it to its own component instead of drawing the wrapper text as a user
    /// turn.
    pub async fn raw_context_messages(&self) -> Vec<cyrup_session::agent_message::AgentMessage> {
        self.manager.lock().await.build_context_raw()
    }

    /// [`raw_context_messages`](Self::raw_context_messages) plus the two derived notices a replay
    /// cannot reconstruct from the messages alone — pi's `renderSessionEntries` flat-map
    /// (`modes/interactive/interactive-mode.ts:3781-3796` @v0.83.0) together with the cache-miss
    /// re-injection its `renderSessionItems` performs (`:3694-3696`, `:3753-3755`).
    ///
    /// **Why this is a session method and not a front-end walk.** The two facts are keyed in index
    /// spaces the front-end never holds at once:
    ///
    /// * [`cyrup_provider::cache_stats::collect_cache_misses`] keys by index into the FLAT entry
    ///   list (its own module doc explains why it cannot key by message), while the replay stream
    ///   is the current branch's post-compaction-admission projection — unrelated index spaces that
    ///   pi bridges with `AssistantMessage` object identity.
    /// * `usage` lives on the compaction / branch-summary ENTRY
    ///   (`cyrup-session/src/entry.rs`); the projected `CompactionSummaryMessage` /
    ///   `BranchSummaryMessage` carry only the summary text, so the cost is gone by the time a
    ///   `Vec<AgentMessage>` reaches the caller.
    ///
    /// [`cyrup_session::manager::SessionManager::build_context_raw_tagged`] carries the
    /// [`cyrup_core::EntryId`] that joins both, under the one manager lock the misses are scanned
    /// under.
    ///
    /// The aborted/errored exclusion on the cache-miss re-injection is pi's own (`:3752`).
    /// Gating on `showCacheMissNotices` is NOT done here: pi gates at each render site, and the
    /// front-end owns that setting's live value.
    pub async fn replay_items(&self) -> Vec<crate::session::ReplayItem> {
        use crate::session::{CompactionCostKind, ReplayItem};
        use cyrup_core::{EntryId, Message, StopReason, Usage};
        use cyrup_session::agent_message::AgentMessage;
        use cyrup_session::entry::{Entry, KnownEntry};
        use std::collections::HashMap;

        let models = self.full_model_registry();
        let mgr = self.manager.lock().await;
        let entries = mgr.entries();
        // The miss scan runs over EVERY entry — a compaction is a scan reset, so restricting it to
        // the branch projection would change the answer (`cache_stats.rs:110-115`).
        let scan = crate::state::cache_scan_entries(entries);
        let misses: HashMap<EntryId, cyrup_provider::cache_stats::CacheMiss> =
            cyrup_provider::cache_stats::collect_cache_misses(&scan, &models)
                .into_iter()
                .filter_map(|(i, miss)| entries.get(i).map(|e| (e.id(), miss)))
                .collect();
        let costs: HashMap<EntryId, (CompactionCostKind, Usage)> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Known(KnownEntry::Compaction { usage: Some(u), .. }) => {
                    Some((e.id(), (CompactionCostKind::Compaction, u.clone())))
                }
                Entry::Known(KnownEntry::BranchSummary { usage: Some(u), .. }) => {
                    Some((e.id(), (CompactionCostKind::BranchSummary, u.clone())))
                }
                _ => None,
            })
            .collect();

        let tagged = mgr.build_context_raw_tagged();
        let mut out = Vec::with_capacity(tagged.len());
        for (id, message) in tagged {
            // Resolved BEFORE the message moves into the stream; emitted after it, which is pi's
            // order at both sites.
            let notice = match &message {
                AgentMessage::Core(Message::Assistant(a))
                    if !matches!(a.stop_reason, StopReason::Aborted | StopReason::Error) =>
                {
                    misses.get(&id).copied().map(ReplayItem::CacheMiss)
                }
                // A summary entry with no `usage`, or one whose summary was empty and therefore
                // projected no message at all, contributes nothing — pi's
                // `entry.usage && messages.length > 0` (`:3791`).
                AgentMessage::CompactionSummary(_) | AgentMessage::BranchSummary(_) => costs
                    .get(&id)
                    .cloned()
                    .map(|(kind, usage)| ReplayItem::CompactionCost { kind, usage }),
                _ => None,
            };
            out.push(ReplayItem::Message(Box::new(message)));
            if let Some(notice) = notice {
                out.push(notice);
            }
        }
        out
    }

    /// The id of the current branch leaf (Pi `sessionManager.getLeafId()`, agent-session.ts:2705).
    /// `None` before any entry exists / after a reset-to-root. Drives the `/tree` overlay's
    /// current-position marker and its `navigate_tree` no-op guard.
    pub async fn leaf_id(&self) -> Option<EntryId> {
        self.manager.lock().await.leaf_id().cloned()
    }

    /// The handle the `read` tool reads to decide whether the ACTIVE model accepts image input
    /// (pi `tools/read.ts`'s non-vision warning). Seeded from the resolved model at build and
    /// re-pushed by `apply_model_change`, so the warning tracks `/model` switches rather than the
    /// startup model.
    #[must_use]
    pub fn read_model_vision(&self) -> &cyrup_tools::config::ModelVisionHandle {
        &self.read_model_vision
    }

    /// The agent's LIVE per-request header overlay (pi `SimpleStreamOptions.headers`, recomputed
    /// per request in `streamFn`, `sdk.ts:318-327`). Tracks the active model via
    /// [`Self::attribution_headers`] on both model-change paths.
    pub async fn agent_headers(&self) -> Option<cyrup_provider::HeaderMap> {
        self.agent.snapshot().await.headers
    }

    /// The agent's current in-memory transcript (includes the streaming partial).
    pub async fn agent_messages(&self) -> Vec<cyrup_agent::AgentMessage> {
        self.agent.snapshot().await.messages
    }

    /// The most recent assistant message text on the current branch (print-mode helper).
    pub async fn last_assistant_text(&self) -> Option<String> {
        self.messages().await.into_iter().rev().find_map(|m| match m {
            Message::Assistant(AssistantMessage { content, .. }) => {
                let text: String = content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if text.is_empty() { None } else { Some(text) }
            }
            _ => None,
        })
    }

    /// The file-based prompt templates discovered for this session (Pi `promptTemplates` getter,
    /// agent-session.ts:880).
    pub fn prompt_templates(
        &self,
    ) -> &cyrup_resources::ResourceSet<cyrup_resources::PromptTemplate> {
        &self.services.resources.prompts
    }

    /// The currently-installed provider's model catalog (Pi `modelRegistry` getter,
    /// agent-session.ts:1412). Returned by value because the underlying provider is now swappable
    /// (see [`ProviderSwap`]); for the cross-provider `/model` list use
    /// [`Self::available_model_catalog`], which spans the full configured registry.
    pub fn model_catalog(&self) -> Vec<cyrup_provider::Model> {
        self.provider.current().models().to_vec()
    }

    /// The session-scoped resource registry (Pi `resourceLoader` getter, agent-session.ts:363).
    pub fn resources(&self) -> &Arc<cyrup_resources::ResourceRegistry> {
        &self.services.resources
    }

    /// Read-only handle to the extension host (Pi `extensionRunner` getter, agent-session.ts:3142).
    pub fn ext_host(&self) -> &Arc<cyrup_ext::ExtensionHost> {
        &self.services.ext_host
    }

    /// Whether any loaded extension handles `kind` (pi `hasExtensionHandlers(eventType: string):
    /// boolean { return this._extensionRunner.hasHandlers(eventType); }`,
    /// `core/agent-session.ts:3334` @v0.83.0). The `:3135` this used to cite is inside an unrelated
    /// usage-totals accumulation loop.
    pub fn has_extension_handlers(&self, kind: cyrup_ext::EventKind) -> bool {
        !self.services.ext_host.dispatcher().no_subscribers(kind)
    }

    /// Load a live wasm extension COMPONENT into this session's host, injecting the session's
    /// [`crate::host_services::LiveHostServices`] as the capability backend (arch-08 §5.6; Pi
    /// `agent-session-services.ts` extension load). This is THE injection seam that retires the
    /// cyrup-ext §08 ledger row: the same `host_services` that drives live model/session/control
    /// state is what the guest's `models`/`session`/`control` imports reach. Behind the `wasm-host`
    /// feature (ON by default — the host is built with the Wasmtime engine). A guest that registers a
    /// slash command via this seam executes through the real run path end-to-end (proven by
    /// `tests/wasm_slash_command.rs`: `prompt("/greet …")` → `_tryExecuteExtensionCommand` → the
    /// guest's `execute-command` export).
    /// EXT-059 — `caps` is REQUIRED, not implied. This used to call
    /// [`cyrup_ext::ExtensionHost::load_wasm`], which is `load_wasm_with_caps(…,
    /// &Capabilities::host_granted())`: a byte-level load with a TOTAL grant, chosen silently by a
    /// function whose name says nothing about capabilities. `ExtensionHost::load_wasm_with_caps`
    /// existed the whole time and had no session-level caller, so no embedder could restrict a
    /// component it loaded through the session without dropping to the raw host.
    ///
    /// `Capabilities::host_granted()` is still the right answer for an EMBEDDER-supplied component
    /// — pi has no capability model at all, so an embedder's own extension is unconditionally
    /// total (`ExtensionHost::load_wasm`'s doc) — it just has to be SAID.
    #[cfg(feature = "wasm-host")]
    pub async fn load_wasm_extension(
        &self,
        id: cyrup_core::ExtensionId,
        bytes: &[u8],
        caps: &cyrup_ext::Capabilities,
    ) -> Result<Arc<cyrup_ext::host::LiveExtension>, SessionServiceError> {
        let services: Arc<dyn cyrup_ext::host::HostServices> = self.services.host_services.clone();
        Ok(self.services.ext_host.load_wasm_with_caps(id, bytes, services, caps).await?)
    }
}
