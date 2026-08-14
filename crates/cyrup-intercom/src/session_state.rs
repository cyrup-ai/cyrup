//! [`SharedIntercomState`] — the per-session state the tools, the inbound event loop, and the seam
//! channels all share: the live [`IntercomClient`], the inbound [`ReplyTracker`], the outbound
//! single-slot [`OutboundReplyWaiter`], the resolved [`IntercomConfig`], and this session's own id.
//!
//! The [`IntercomClient`] is created on `SessionStart` (after the broker is health-connectable) and
//! stashed here; tools/seams clone the `Arc` out under a short lock (never held across `.await`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_ext::HostServices;

use crate::config::IntercomConfig;
use crate::error::{IntercomError, Result};
use crate::reply_tracker::{OutboundReplyWaiter, ReplyTracker};
use crate::transport::client::{IntercomClient, SendOptions};

/// The three context-usage fields of a `presence` frame, in the tri-state the wire has
/// (`v0.9.2 types.ts:86`): `None` omits the key, `Some(None)` sends an explicit `null` (the broker
/// CLEARS the field), `Some(Some(n))` sets it. Produced by
/// [`SharedIntercomState::current_context_usage`], which is the port of pi's spread-in-place
/// `...currentContextUsage()` (`v0.9.2 index.ts:816,847`) — Rust has no object spread, so the three
/// keys travel as one value.
#[derive(Debug, Default)]
pub struct PresenceContext {
    /// `contextPct` (0..100, rounded).
    pub pct: Option<Option<serde_json::Number>>,
    /// `contextTokens`.
    pub tokens: Option<Option<serde_json::Number>>,
    /// `contextWindow`.
    pub window: Option<Option<serde_json::Number>>,
}

/// The shared, session-scoped intercom state.
pub struct SharedIntercomState {
    client: Mutex<Option<Arc<IntercomClient>>>,
    /// The live `HostServices` backend, late-bound via P-1 Route B (`set_host_services` before
    /// `init`, the port doc §4.1). The SAME `Arc` the session mutates via `set_ui_sink`/manager
    /// attach, so the inbound surface + ClarifyChannel human answer observe those through it even
    /// though they run OUTSIDE any `HostCtx`. `None` until the builder late-binds it (or in a
    /// headless/degraded session) → the human surface degrades to a no-op, never blocks.
    host_services: Mutex<Option<Arc<dyn HostServices>>>,
    /// Whether this session has an interactive UI (pi `hasUI`, `index.ts:739-758`). Captured ONCE
    /// from the live `HostCtx::has_ui` at `SessionStart` (a static per-session property) and read by
    /// the inbound delivery policy ([`crate::inbound`]): an interactive session drives/steers a turn
    /// over an inbound message; a non-interactive (`!has_ui`) session instead sends the sender the
    /// "running in non-interactive mode" busy auto-reply. Defaults `false` (no UI) until the
    /// `SessionStart` handler sets it — a headless/degraded session then takes the auto-reply branch.
    has_ui: AtomicBool,
    /// Tool calls currently in flight, keyed by `ToolCallId` (pi `activeTools`,
    /// `v0.10.1 index.ts:525`), in INSERTION order.
    ///
    /// Order is load-bearing: `currentStatus` reads `activeTools.values().next().value`
    /// (`v0.10.1 index.ts:677`), and a JS `Map` iterates in insertion order — so with two overlapping
    /// calls the status names the one that started FIRST, and it keeps naming it until that one
    /// ends. A `HashMap` would name an arbitrary one; hence a `Vec` of pairs rather than a map.
    /// Cleared wholesale on `agent_start`/`agent_end` (`:1430`, `:1452`) and on shutdown (`:1409`).
    active_tools: Mutex<Vec<(cyrup_core::ToolCallId, String)>>,
    /// Whether an agent run is in flight (pi `agentRunning`, `v0.10.1 index.ts:524`) — the
    /// `thinking` vs `idle` axis of `currentStatus` once no tool is active.
    agent_running: AtomicBool,
    /// This session's child-orchestrator metadata, published by
    /// [`crate::extension::IntercomExtension::new`] so [`Self::sync_presence_identity`] derives the
    /// same presence name `crate::connect::build_registration` did. Upstream needs no equivalent:
    /// `buildPresenceIdentity` is a closure over the extension scope and every caller is inside it.
    presence_metadata: Mutex<Option<crate::identity::ChildOrchestratorMetadata>>,
    /// `lastPresenceName` + `lastPresenceRuntimeFallbackAlias` (`v0.10.1 index.ts:513`, `:514`) —
    /// what the name poll diffs against so it emits a `Presence` frame on a CHANGE, not once per
    /// tick.
    last_presence_identity: Mutex<Option<crate::connect::PresenceIdentity>>,
    /// The `namePollTimer` task (`v0.10.1 index.ts:817-831`). Cancelled on `session_shutdown`
    /// (`clearNamePollTimer()`, `:1407`).
    name_poll_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The connection supervisor's state — pi's `reconnectTimer`/`reconnectAttempt`/`shuttingDown`/
    /// `runtimeGeneration` closure vars (`index.ts:439-449`). Driven by [`crate::connect`]:
    /// `ensure_connected`/`schedule_reconnect` recover the broker connection after a drop instead of
    /// leaving [`Self::client`] empty for the rest of the session.
    pub connect: crate::connect::ConnectSupervisor,
    /// Inbound ask tracking (`ReplyTracker`).
    pub tracker: Mutex<ReplyTracker>,
    /// The outbound single-slot reply waiter (`replyWaiter`).
    pub waiter: OutboundReplyWaiter,
    /// The resolved intercom config.
    pub config: IntercomConfig,
    /// The ask timeout (default 10 min).
    pub ask_timeout_ms: u64,
    /// This session's working directory (captured at construction, like `SubagentsExtension`; used
    /// for the `intercom{list}` "same cwd" tag).
    pub cwd: std::path::PathBuf,
}

impl SharedIntercomState {
    /// Build the shared state with no client connected yet.
    #[must_use]
    pub fn new(config: IntercomConfig, ask_timeout_ms: u64, cwd: std::path::PathBuf) -> Self {
        Self {
            client: Mutex::new(None),
            host_services: Mutex::new(None),
            has_ui: AtomicBool::new(false),
            active_tools: Mutex::new(Vec::new()),
            agent_running: AtomicBool::new(false),
            presence_metadata: Mutex::new(None),
            last_presence_identity: Mutex::new(None),
            name_poll_task: Mutex::new(None),
            connect: crate::connect::ConnectSupervisor::default(),
            tracker: Mutex::new(ReplyTracker::new(ask_timeout_ms)),
            waiter: OutboundReplyWaiter::new(),
            config,
            ask_timeout_ms,
            cwd,
        }
    }

    /// Late-bind the live `HostServices` backend (P-1 Route B; called from
    /// [`crate::extension::IntercomExtension`]'s `set_host_services`, which the builder invokes via
    /// `load_native_with_services` BEFORE `init`). Idempotent: a session rebuild rebinds the same
    /// shared `Arc`.
    pub fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.host_services.lock().unwrap_or_else(|e| e.into_inner()) = Some(services);
    }

    /// The live `HostServices` backend, if bound (the inbound surface + ClarifyChannel human answer
    /// load it per call and degrade to a no-op when absent).
    #[must_use]
    pub fn host_services(&self) -> Option<Arc<dyn HostServices>> {
        self.host_services.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Record whether this session has an interactive UI (pi `hasUI`). Called ONCE from the
    /// `SessionStart` handler with the live `HostCtx::has_ui`; the inbound delivery policy reads it
    /// via [`Self::has_ui`].
    pub fn set_has_ui(&self, has_ui: bool) {
        self.has_ui.store(has_ui, Ordering::SeqCst);
    }

    /// Whether this session has an interactive UI (pi `hasUI`, `index.ts:739-758`) — the inbound
    /// idle-vs-busy delivery policy's static gate. `false` (the default) until the `SessionStart`
    /// handler binds it, so a headless/degraded session takes the non-interactive auto-reply branch.
    #[must_use]
    pub fn has_ui(&self) -> bool {
        self.has_ui.load(Ordering::SeqCst)
    }

    /// Whether NO agent run is currently in flight (pi `ctx.isIdle()`, `index.ts:745`) — the FIRST
    /// axis of the inbound delivery policy, ahead of [`Self::has_ui`]. Read live off the bound
    /// `HostServices` backend on every inbound message (an agent can start/finish between two of
    /// them). With no backend bound (headless/degraded, or a session whose host never attached) the
    /// answer is `true` — the same "no live session attached ⇒ nothing is running" default
    /// `HostServices::is_idle` itself returns, so a degraded session delivers rather than queueing
    /// forever.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.host_services().is_none_or(|services| services.is_idle())
    }

    /// `activeTools.set(event.toolCallId, event.toolName)` (`v0.10.1 index.ts:1437`). Re-setting an
    /// existing id updates the name in place and keeps its original position, exactly as a JS `Map`
    /// does.
    pub fn tool_started(&self, call_id: cyrup_core::ToolCallId, name: String) {
        let mut tools = self.active_tools.lock().unwrap_or_else(|e| e.into_inner());
        match tools.iter_mut().find(|(id, _)| *id == call_id) {
            Some(slot) => slot.1 = name,
            None => tools.push((call_id, name)),
        }
    }

    /// `activeTools.delete(event.toolCallId)` (`v0.10.1 index.ts:1444`).
    pub fn tool_ended(&self, call_id: &cyrup_core::ToolCallId) {
        self.active_tools.lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _)| id != call_id);
    }

    /// `activeTools.clear()` (`v0.10.1 index.ts:1430`, `:1452`, `:1409`) plus the `agentRunning`
    /// flag those same three sites set — the two always move together upstream.
    pub fn set_agent_running(&self, running: bool) {
        self.agent_running.store(running, Ordering::SeqCst);
        self.active_tools.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// `currentStatus()` (`v0.10.1 index.ts:676-680`, 5 lines):
    ///
    /// ```text
    /// const activeToolName = activeTools.values().next().value;
    /// const lifecycleStatus = activeToolName ? `tool:${activeToolName}` : agentRunning ? "thinking" : "idle";
    /// return config.status ? `${lifecycleStatus} · ${config.status}` : lifecycleStatus;
    /// ```
    ///
    /// `activeTools.values().next().value` is the FIRST still-running tool, not the most recent —
    /// so ending one of two overlapping calls leaves the status on a tool rather than snapping back
    /// to `thinking`, which is the whole reason the map exists.
    #[must_use]
    pub fn current_status(&self) -> String {
        let active_tool =
            self.active_tools.lock().unwrap_or_else(|e| e.into_inner()).first().map(|(_, n)| n.clone());
        let lifecycle = match active_tool {
            Some(name) => format!("tool:{name}"),
            None if self.agent_running.load(Ordering::SeqCst) => "thinking".to_string(),
            None => "idle".to_string(),
        };
        match &self.config.status {
            // JS truthiness: an empty configured suffix is falsy and adds no ` · ` separator.
            Some(suffix) if !suffix.trim().is_empty() => format!("{lifecycle} · {suffix}"),
            _ => lifecycle,
        }
    }

    /// pi `currentContextUsage()` (`v0.9.2 index.ts:790-808`, 19 lines incl. its 5-line comment):
    ///
    /// ```text
    /// const usage = getLiveContext()?.getContextUsage?.();
    /// if (!usage) return {};
    /// const result = {
    ///   contextPct: typeof usage.percent === "number" && Number.isFinite(usage.percent) ? Math.round(usage.percent) : null,
    ///   contextTokens: typeof usage.tokens === "number" && Number.isFinite(usage.tokens) ? usage.tokens : null,
    /// };
    /// if (typeof usage.contextWindow === "number" && usage.contextWindow > 0) result.contextWindow = usage.contextWindow;
    /// return result;
    /// ```
    ///
    /// Note the two tiers, which are what the tri-state on
    /// [`IntercomClient::update_presence_with_context`] exists to carry:
    /// - no usage at all → **omit** all three keys (the broker leaves the peer's view untouched);
    /// - usage present but the token count unknown → send explicit **`null`** for
    ///   `contextPct`/`contextTokens`, which CLEARS a peer's stale value instead of freezing the
    ///   pre-compaction percentage (upstream's own comment, `v0.9.2 index.ts:791-793`; broker side at
    ///   `v0.9.2 broker/broker.ts:922-924`).
    ///
    /// # Shape mapping
    ///
    /// pi's `ContextUsage` is `{tokens: number|null, contextWindow: number, percent: number|null}`
    /// (`pi v0.84.1 coding-agent/src/core/extensions/types.ts:288-294`). cyrup's
    /// `HostServices::context_usage()` deliberately answers in cyrup's own spelling,
    /// `{usedTokens, contextWindow, fraction}` — a KNOWN, documented divergence
    /// (`cyrup-session-svc/src/state.rs:69-75`: "Converging those onto Pi's spelling is a separate
    /// divergence"). It lives in another crate, so this reads that shape and translates:
    ///
    /// - `contextWindow == 0` ⇒ pi's `getContextUsage()` returns `undefined` outright
    ///   (`pi v0.84.1 agent-session.ts:3178-3179`: `if (contextWindow <= 0) return undefined;`), so
    ///   this returns "omit everything", exactly as `if (!usage) return {}` does.
    /// - `usedTokens == 0` ⇒ pi's `tokens: null` / `percent: null`. This is not an approximation:
    ///   cyrup's `ContextUsage::from_last_assistant` yields `used_tokens == 0` precisely when there
    ///   is no usable assistant usage to read (`cyrup-session-svc/src/state.rs:168-180`), which is
    ///   the same condition pi's post-compaction check tests — `contextTokens > 0` over the
    ///   post-compaction assistants, else `{ tokens: null, contextWindow, percent: null }`
    ///   (`pi v0.84.1 agent-session.ts:3196-3207`).
    /// - `contextPct` is computed from `usedTokens`/`contextWindow` rather than from cyrup's
    ///   `fraction`, because `fraction` is clamped to `[0, 1]`
    ///   (`cyrup-session-svc/src/state.rs:164`) while pi's `percent` is not — an over-window session
    ///   must report pi's `104`, not a clamped `100`. Integer arithmetic (`u128`) rather than f64
    ///   both matches `Math.round`'s round-half-up on non-negative inputs exactly and avoids a
    ///   lossy float cast.
    #[must_use]
    pub fn current_context_usage(&self) -> PresenceContext {
        let Some(services) = self.host_services() else {
            return PresenceContext::default();
        };
        let usage = services.context_usage();
        let Some(obj) = usage.as_object() else {
            return PresenceContext::default();
        };
        // `contextWindow <= 0` ⇒ pi has no usage object at all ⇒ omit all three keys.
        let window = obj.get("contextWindow").and_then(serde_json::Value::as_u64).unwrap_or(0);
        if window == 0 {
            return PresenceContext::default();
        }
        let tokens = obj.get("usedTokens").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let (pct, tokens) = if tokens == 0 {
            // pi `{ tokens: null, percent: null }` — send the CLEAR, do not omit.
            (Some(None), Some(None))
        } else {
            let pct = u64::try_from((u128::from(tokens) * 100 + u128::from(window) / 2) / u128::from(window))
                .unwrap_or(u64::MAX);
            (Some(Some(serde_json::Number::from(pct))), Some(Some(serde_json::Number::from(tokens))))
        };
        PresenceContext { pct, tokens, window: Some(Some(serde_json::Number::from(window))) }
    }

    /// `startNamePoll()` (`v0.10.1 index.ts:817-831`, 15 lines):
    ///
    /// ```text
    /// clearNamePollTimer();
    /// const initialIdentity = currentSessionId ? buildPresenceIdentity(pi, …) : null;
    /// lastPresenceName = initialIdentity?.name ?? null;
    /// lastPresenceRuntimeFallbackAlias = initialIdentity?.runtimeFallbackAlias ?? null;
    /// namePollTimer = setInterval(() => {
    ///   if (!currentSessionId || !getLiveContext()) return;
    ///   const identity = buildPresenceIdentity(pi, …);
    ///   if (identity.name !== lastPresenceName || identity.runtimeFallbackAlias !== lastPresenceRuntimeFallbackAlias) {
    ///     syncPresenceIdentity(currentSessionId);
    ///   }
    /// }, getNamePollMs());
    /// namePollTimer.unref?.();
    /// ```
    ///
    /// The DIFF is the point: a session renamed by `/name`, a branch switch or a title change is
    /// picked up within one interval, and an unchanged one sends nothing — upstream deliberately
    /// does not heartbeat the name. This is upstream's third and last name-sync point; the other two
    /// (`turn_start`, every `intercom` tool call) are the cheap 80% and need no timer at all.
    pub fn start_name_poll(self: &Arc<Self>) {
        let metadata = self.presence_metadata.lock().unwrap_or_else(|e| e.into_inner()).clone();
        *self.last_presence_identity.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(crate::connect::presence_identity(self, metadata.as_ref()));
        let interval = std::time::Duration::from_millis(crate::identity::name_poll_ms());
        let state = self.clone();
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // `setInterval` does not fire immediately; tokio's first tick does.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                // `if (!currentSessionId || !getLiveContext()) return;` — a poll against a torn-down
                // or not-yet-connected runtime does nothing (and, unlike upstream's `return` from
                // one callback, keeps ticking).
                if state.client().is_none() {
                    continue;
                }
                let metadata =
                    state.presence_metadata.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let identity = crate::connect::presence_identity(&state, metadata.as_ref());
                let changed = state
                    .last_presence_identity
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    != Some(&identity);
                if changed {
                    state.sync_presence_identity();
                }
            }
        });
        // `clearNamePollTimer()` at the head of `startNamePoll` (`v0.10.1 index.ts:818`): a runtime
        // replacement must not leave two pollers running.
        let previous = std::mem::replace(
            &mut *self.name_poll_task.lock().unwrap_or_else(|e| e.into_inner()),
            Some(handle),
        );
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    /// `clearNamePollTimer()` (`v0.10.1 index.ts:1407`, on `session_shutdown`).
    pub fn stop_name_poll(&self) {
        if let Some(task) = self.name_poll_task.lock().unwrap_or_else(|e| e.into_inner()).take() {
            task.abort();
        }
        *self.last_presence_identity.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// `currentSessionTargetMatches(to, resolvedTo?, activeClient?)`
    /// (`v0.10.1 index.ts:850-863`, 14 lines):
    ///
    /// ```text
    /// const targets = new Set<string>();
    /// addTarget(currentSessionId); addTarget(currentIntercomSessionId);
    /// addTarget(activeClient?.sessionId); addTarget(pi.getSessionName());
    /// if (currentSessionId) addTarget(buildPresenceIdentity(pi, …).name);
    /// return Boolean(resolvedTo && activeClient?.sessionId && resolvedTo === activeClient.sessionId)
    ///   || targets.has(to.trim().toLowerCase());
    /// ```
    ///
    /// Consulted TWICE around one relay — before resolving and again after (`v0.10.1 index.ts:1316`,
    /// `:1335`) — because a name can only be recognised as "me" pre-resolution while a resolved id
    /// can only be compared post-resolution.
    ///
    /// Note the resolved-id arm is **case-sensitive and un-trimmed** upstream (a raw `===`), while
    /// the set membership test lower-cases and trims; both are reproduced.
    #[must_use]
    pub fn current_session_target_matches(&self, to: &str, resolved_to: Option<&str>) -> bool {
        let self_id = self.self_session_id();
        if let (Some(resolved), Some(active)) = (resolved_to, self_id.as_deref())
            && resolved == active
        {
            return true;
        }
        let mut targets: Vec<String> = Vec::new();
        let mut add = |target: Option<String>| {
            if let Some(t) = target {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    targets.push(trimmed.to_lowercase());
                }
            }
        };
        let services = self.host_services();
        add(services.as_ref().and_then(|s| s.session_id()));
        add(self_id);
        add(services.as_ref().and_then(|s| s.session_name()));
        let metadata = self.presence_metadata.lock().unwrap_or_else(|e| e.into_inner()).clone();
        add(crate::connect::presence_identity_name(self, metadata.as_ref()));
        targets.contains(&to.trim().to_lowercase())
    }

    /// Publish this session's child-orchestrator metadata for [`Self::sync_presence_identity`].
    pub fn set_presence_metadata(&self, metadata: Option<crate::identity::ChildOrchestratorMetadata>) {
        *self.presence_metadata.lock().unwrap_or_else(|e| e.into_inner()) = metadata;
    }

    /// `syncPresenceIdentity(sessionId)` (`v0.10.1 index.ts:808-815`) — re-derive the presence NAME
    /// from the live host and send it with the derived status and the live context usage.
    ///
    /// Reachable from the `intercom` tool as well as the lifecycle arms, because upstream calls it
    /// at the head of every `intercom` tool `execute` (`v0.10.1 index.ts:1853`), immediately after
    /// `ensureConnected("tool")` — a tool call is a point where the caller is about to be shown, or
    /// to hand out, this session's address.
    pub fn sync_presence_identity(&self) {
        let Some(client) = self.client() else {
            return;
        };
        let metadata = self.presence_metadata.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let identity = crate::connect::presence_identity(self, metadata.as_ref());
        // `lastPresenceName = identity.name; lastPresenceRuntimeFallbackAlias = …`
        // (`v0.10.1 index.ts:813-814`) — every sync updates the poll's baseline, so a rename that
        // reached the broker through `turn_start` or a tool call does not also fire from the poll.
        *self.last_presence_identity.lock().unwrap_or_else(|e| e.into_inner()) = Some(identity.clone());
        let usage = self.current_context_usage();
        client.update_presence_full(
            identity.name.clone(),
            identity.name.as_ref().map(|_| identity.runtime_fallback_alias),
            Some(self.current_status()),
            None,
            usage.pct,
            usage.tokens,
            usage.window,
        );
    }

    /// Resolve a name/id/unique-prefix `to` to a single session id against the live session list —
    /// `resolveSessionTarget` (`v0.10.1 index.ts:1140-1165`). `Ok(None)` = no match.
    ///
    /// This is the CLIENT-side resolver, and it is deliberately not
    /// [`crate::broker::routing::find_session_ids`] (the broker's `findSessions`): upstream raises
    /// **two different** errors here, because the caller needs to know which kind of ambiguity it
    /// hit and what to type instead.
    ///
    /// ```text
    /// byName.length > 1  → `Multiple sessions named "${nameOrId}" are connected. Address one by the
    ///                       id shown in parentheses by "list" (${ids}).`   // ids = sessionIdPrefixes
    /// byIdPrefix.length > 1 → `Multiple sessions match ID prefix "${nameOrId}". Use a longer session
    ///                       ID prefix.`
    /// ```
    ///
    /// The candidate ids in the first message are **`sessionIdPrefixes` values**, not raw uuids and
    /// not 8-char slices (`v0.10.1 index.ts:1149-1150`) — i.e. exactly the strings `intercom{list}`
    /// printed, so the remedy the message names is reachable from the roster the model already saw.
    ///
    /// # Errors
    /// [`IntercomError::Client`] on a `list` failure or an ambiguous match.
    pub async fn resolve_target(&self, client: &Arc<IntercomClient>, name_or_id: &str) -> Result<Option<String>> {
        let sessions = client.list_sessions().await?;

        if let Some(by_id) = sessions.iter().find(|s| s.id == name_or_id) {
            return Ok(Some(by_id.id.clone()));
        }

        let lower_name = name_or_id.to_lowercase();
        let by_name: Vec<&crate::transport::protocol::SessionInfo> = sessions
            .iter()
            .filter(|s| s.name.as_deref().map(str::to_lowercase) == Some(lower_name.clone()))
            .collect();
        if by_name.len() > 1 {
            let prefixes =
                crate::identity::session_id_prefixes(sessions.iter().map(|s| s.id.as_str()));
            let ids = by_name
                .iter()
                .map(|s| prefixes.get(&s.id).cloned().unwrap_or_else(|| s.id.clone()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(IntercomError::Client(format!(
                "Multiple sessions named \"{name_or_id}\" are connected. Address one by the id shown in parentheses by \"list\" ({ids})."
            )));
        }
        if let Some(only) = by_name.first() {
            return Ok(Some(only.id.clone()));
        }

        let by_id_prefix: Vec<&crate::transport::protocol::SessionInfo> =
            sessions.iter().filter(|s| s.id.starts_with(name_or_id)).collect();
        if by_id_prefix.len() > 1 {
            return Err(IntercomError::Client(format!(
                "Multiple sessions match ID prefix \"{name_or_id}\". Use a longer session ID prefix."
            )));
        }
        Ok(by_id_prefix.first().map(|s| s.id.clone()))
    }

    /// Stash the live client (on connect) or clear it (on disconnect).
    pub fn set_client(&self, client: Option<Arc<IntercomClient>>) {
        *self.client.lock().unwrap_or_else(|e| e.into_inner()) = client;
    }

    /// The live client, if connected.
    #[must_use]
    pub fn client(&self) -> Option<Arc<IntercomClient>> {
        self.client.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// This session's own broker-assigned id, if connected (for the self-target guard).
    #[must_use]
    pub fn self_session_id(&self) -> Option<String> {
        self.client().and_then(|c| c.session_id())
    }

    /// Issue a blocking ask over the broker and await the reply (`waitForReply` +
    /// `client.send(expectsReply)`, `index.ts:1295-1373,1613-1616`): register the outbound single
    /// slot, send with `expects_reply`, then race the reply against the ask timeout and the tool's
    /// cancellation. On every non-reply exit the slot is cleared and the broker ask edge cancelled.
    ///
    /// # Errors
    /// [`IntercomError::Client`] on "Already waiting for a reply", a non-delivered send, a timeout,
    /// or cancellation.
    pub async fn ask_and_wait(
        &self,
        client: &Arc<IntercomClient>,
        target: &str,
        question_id: String,
        text: String,
        attachments: Option<Vec<crate::transport::protocol::Attachment>>,
        cancel: &cyrup_core::CancelToken,
    ) -> Result<String> {
        let message = self
            .ask_and_wait_with_reply_to(client, target, question_id, text, attachments, None, cancel)
            .await?;
        // Inline the reply's attachments into the visible body, exactly as pi does
        // (`replyText + formatAttachments(replyMessage.content.attachments)`,
        // `v0.10.1 index.ts:2168-2170`, `index.ts:1354-1357`) — never silently drop them.
        Ok(inline_reply_attachments(message.content.text, message.content.attachments.as_deref()))
    }

    /// [`Self::ask_and_wait`] carrying the caller's `replyTo` — pi `index.ts:2154-2160`
    /// (v0.7.0 `index.ts:1626-1632`):
    ///
    /// ```text
    /// connectedClient.send(sendTo, { messageId: questionId, text: message, attachments, replyTo, expectsReply: true })
    /// ```
    ///
    /// `replyTo` is load-bearing on the ask path, not decoration: cyrup's own broker rejects a send
    /// whose `reply_to` matches no ask edge, so counter-asking a peer's pending ask without it comes
    /// back as `Reply target does not match a pending ask` — a tool that advertises `replyTo` and
    /// then drops it turns into an apparent broker bug.
    ///
    /// Returns the reply **`Message`**, not its text: `intercom{ask}`'s `intercom_received` audit
    /// entry records the reply's own `messageId`, its `attachments` and the SENDER's `timestamp`
    /// (`v0.10.1 index.ts:2171-2176`), none of which survive a pre-flattened string.
    ///
    /// # Errors
    /// As [`Self::ask_and_wait`].
    #[allow(clippy::too_many_arguments)]
    pub async fn ask_and_wait_with_reply_to(
        &self,
        client: &Arc<IntercomClient>,
        target: &str,
        question_id: String,
        text: String,
        attachments: Option<Vec<crate::transport::protocol::Attachment>>,
        reply_to: Option<String>,
        cancel: &cyrup_core::CancelToken,
    ) -> Result<crate::transport::protocol::Message> {
        // Single-slot guard (`if replyWaiter → "Already waiting for a reply"`).
        let rx = self
            .waiter
            .register(target.to_string(), question_id.clone())
            .map_err(IntercomError::Client)?;

        let send_result = client
            .send(target, SendOptions {
                text,
                attachments,
                reply_to,
                expects_reply: Some(true),
                message_id: Some(question_id.clone()),
            })
            .await;

        match send_result {
            Ok(result) if result.delivered => {}
            Ok(result) => {
                self.waiter.clear_matching(&question_id);
                return Err(IntercomError::Client(
                    result.reason.unwrap_or_else(|| "ask was not delivered".to_string()),
                ));
            }
            Err(e) => {
                self.waiter.clear_matching(&question_id);
                return Err(e);
            }
        }

        let timeout = Duration::from_millis(self.ask_timeout_ms);
        tokio::select! {
            reply = rx => {
                match reply {
                    Ok(Ok(message)) => Ok(message),
                    // The slot was FAILED out from under us — pi `rejectReplyWaiter` on the client
                    // `disconnected` edge (`index.ts:783-784`) / session replace / shutdown. Surface
                    // that reason verbatim instead of hanging until the ask timeout.
                    Ok(Err(reason)) => {
                        client.cancel_ask(&question_id);
                        Err(IntercomError::Client(reason))
                    }
                    Err(_) => {
                        // The slot's sender was dropped (cleared elsewhere).
                        self.waiter.clear_matching(&question_id);
                        client.cancel_ask(&question_id);
                        Err(IntercomError::Client("reply waiter cancelled".to_string()))
                    }
                }
            }
            () = tokio::time::sleep(timeout) => {
                self.waiter.clear_matching(&question_id);
                client.cancel_ask(&question_id);
                Err(IntercomError::Client(format!(
                    "No reply from \"{target}\" within {}",
                    describe_timeout(self.ask_timeout_ms)
                )))
            }
            () = cancel.cancelled() => {
                self.waiter.clear_matching(&question_id);
                client.cancel_ask(&question_id);
                Err(IntercomError::Client("Cancelled".to_string()))
            }
        }
    }
}

/// Inline a reply's attachments into its visible text (pi `replyText + formatAttachments(...)`,
/// `index.ts:1646-1649` (ask) and `index.ts:1354-1357` (contact_supervisor)) — attachments the
/// replying session sent back must never be silently dropped.
fn inline_reply_attachments(text: String, attachments: Option<&[crate::transport::protocol::Attachment]>) -> String {
    let attachment_text = attachments
        .filter(|a| !a.is_empty())
        .map(crate::inbound::format_attachments)
        .unwrap_or_default();
    format!("{text}{attachment_text}")
}

/// `askTimeoutMs % 60000 === 0 ? "N minutes" : "Nms"` (`index.ts:471`).
fn describe_timeout(ask_timeout_ms: u64) -> String {
    if ask_timeout_ms.is_multiple_of(60_000) {
        format!("{} minutes", ask_timeout_ms / 60_000)
    } else {
        format!("{ask_timeout_ms}ms")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn describe_timeout_formats_minutes_and_ms() {
        assert_eq!(describe_timeout(600_000), "10 minutes");
        assert_eq!(describe_timeout(60_000), "1 minutes");
        assert_eq!(describe_timeout(5000), "5000ms");
    }

    /// `currentStatus()` (`v0.10.1 index.ts:676-680`). Two independent regressions are pinned:
    ///
    /// * ICOM-009 — with two OVERLAPPING tool calls, ending the first must NOT reset presence to
    ///   `thinking` while the second is still running. Pre-fix each lifecycle arm passed its own
    ///   literal `base` string and there was no per-`ToolCallId` map at all, so `ToolExecEnd` always
    ///   said `thinking`.
    /// * ICOM-021 — the configured suffix is APPENDED to a lifecycle status, never used alone.
    #[test]
    fn current_status_tracks_overlapping_tools_and_appends_the_config_suffix() {
        use cyrup_core::ToolCallId;
        let config = IntercomConfig { status: Some("reviewing".to_string()), ..IntercomConfig::default() };
        let s = SharedIntercomState::new(config, 600_000, std::path::PathBuf::from("/w"));

        assert_eq!(s.current_status(), "idle · reviewing");
        s.set_agent_running(true);
        assert_eq!(s.current_status(), "thinking · reviewing");

        let a = ToolCallId::from("call-a");
        let b = ToolCallId::from("call-b");
        s.tool_started(a.clone(), "bash".to_string());
        s.tool_started(b.clone(), "read".to_string());
        // `activeTools.values().next().value` is the FIRST still-running tool, in insertion order.
        assert_eq!(s.current_status(), "tool:bash · reviewing");
        s.tool_ended(&a);
        assert_eq!(
            s.current_status(),
            "tool:read · reviewing",
            "ending one of two overlapping calls must not claim the session went back to thinking"
        );
        s.tool_ended(&b);
        assert_eq!(s.current_status(), "thinking · reviewing");

        // `agent_end` clears both axes at once (`v0.10.1 index.ts:1451-1452`).
        s.tool_started(a, "bash".to_string());
        s.set_agent_running(false);
        assert_eq!(s.current_status(), "idle · reviewing");
    }

    /// JS truthiness: `config.status ? … : lifecycleStatus` — an absent or blank suffix adds no
    /// ` · ` separator.
    #[test]
    fn current_status_without_a_configured_suffix_is_the_bare_lifecycle_status() {
        let s = SharedIntercomState::new(IntercomConfig::default(), 600_000, std::path::PathBuf::from("/w"));
        assert_eq!(s.current_status(), "idle");
        let blank = IntercomConfig { status: Some("   ".to_string()), ..IntercomConfig::default() };
        let s = SharedIntercomState::new(blank, 600_000, std::path::PathBuf::from("/w"));
        assert_eq!(s.current_status(), "idle");
    }

    /// ICOM-034 / `currentSessionTargetMatches` (`v0.10.1 index.ts:850-863`). With no live client
    /// and no host services there is nothing to match, which is the degraded default; the trimmed,
    /// lower-cased set membership is what the seams rely on.
    #[test]
    fn current_session_target_matches_is_false_without_an_identity() {
        let s = SharedIntercomState::new(IntercomConfig::default(), 600_000, std::path::PathBuf::from("/w"));
        assert!(!s.current_session_target_matches("anyone", None));
        // The resolved-id arm needs a live client id, so it cannot fire either.
        assert!(!s.current_session_target_matches("anyone", Some("some-id")));
    }

    #[test]
    fn set_and_clear_client() {
        let state = SharedIntercomState::new(IntercomConfig::default(), 600_000, std::path::PathBuf::from("/w"));
        assert!(state.client().is_none());
        assert!(state.self_session_id().is_none());
    }

    /// Regression proof for the "ask/contact_supervisor replies drop attachments" divergence
    /// (pi `index.ts:1646-1649,1354-1357`): before the fix, `ask_and_wait` returned
    /// `message.content.text` verbatim, discarding `content.attachments` entirely. The reply text
    /// must now carry the same `Attachment: name\n content` block pi's `formatAttachments`
    /// inlines (`v0.10.1 index.ts:95-105`; the delimiter was `📎 {name}` until v0.10.0's
    /// `633e782` "refactor: deslop intercom protocol cleanup" renamed it).
    #[test]
    fn inline_reply_attachments_appends_pi_formatted_block() {
        use crate::transport::protocol::{Attachment, AttachmentKind};

        let text = inline_reply_attachments(
            "Looks good".to_string(),
            Some(&[Attachment {
                kind: AttachmentKind::Snippet,
                name: "patch.diff".to_string(),
                content: "+1 line".to_string(),
                language: Some("diff".to_string()),
                extra: Default::default(),
            }]),
        );
        assert_eq!(text, "Looks good\n\n---\nAttachment: patch.diff\n~~~diff\n+1 line\n~~~");
    }

    /// No attachments ⇒ the reply text passes through unchanged (pi: `replyAttachments = ""` when
    /// `replyMessage.content.attachments?.length` is falsy).
    #[test]
    fn inline_reply_attachments_passes_through_when_none() {
        assert_eq!(inline_reply_attachments("no attachments here".to_string(), None), "no attachments here");
        assert_eq!(inline_reply_attachments("empty vec".to_string(), Some(&[])), "empty vec");
    }
}
