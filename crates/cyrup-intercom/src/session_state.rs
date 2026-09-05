//! [`SharedIntercomState`] — the per-session state the tools, the inbound event loop, and the seam
//! channels all share: the live [`IntercomClient`], the inbound [`ReplyTracker`], the outbound
//! single-slot [`OutboundReplyWaiter`], the resolved [`IntercomConfig`], and this session's own id.
//!
//! The [`IntercomClient`] is created on `SessionStart` (after the broker is health-connectable) and
//! stashed here; tools/seams clone the `Arc` out under a short lock (never held across `.await`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_ext::HostServices;

use crate::config::IntercomConfig;
use crate::error::{IntercomError, Result};
use crate::reply_tracker::{OutboundReplyWaiter, ReplyTracker};
use crate::transport::client::{IntercomClient, SendOptions};
use crate::transport::protocol::{
    MessageControl, MessageControlAction, MessageReceipt, MessageReceiptStatus, now_ms,
};

/// `INBOUND_MESSAGE_DEDUPE_MAX` (`v0.10.1 index.ts:32`).
const INBOUND_MESSAGE_DEDUPE_MAX: usize = 1000;
/// `INBOUND_MESSAGE_DEDUPE_RETENTION_MS` (`v0.10.1 index.ts:33`) — one hour.
const INBOUND_MESSAGE_DEDUPE_RETENTION_MS: u64 = 60 * 60 * 1000;

/// `seenInboundMessages: Map<string, number>` (`v0.10.1 index.ts:527`) — the `(from.id, message.id)`
/// pairs this session has already accepted, with the epoch-ms it first saw each.
///
/// A JS `Map` is BOTH a hash lookup and an insertion-ordered list, and `hasSeenInboundMessage` uses
/// both halves: `has(key)` is the dedupe test, and `keys().next().value` is the OLDEST key, which is
/// what the size cap evicts (`:543-547`). A bare `HashMap` would evict an arbitrary entry, which
/// turns the cap from "forget the oldest" into "forget one at random" — so the insertion order is
/// carried explicitly in `order`.
#[derive(Debug, Default)]
struct SeenInboundMessages {
    /// Insertion order, oldest first (`seenInboundMessages.keys()`).
    order: VecDeque<String>,
    /// key → first-seen epoch-ms.
    seen: HashMap<String, u64>,
}

impl SeenInboundMessages {
    /// `hasSeenInboundMessage(from, message, now)` (`v0.10.1 index.ts:532-548`): sweep expired
    /// entries, then test-and-insert, then evict down to the cap. Returns whether this pair had
    /// already been accepted.
    fn test_and_insert(&mut self, key: String, now: u64) -> bool {
        // The retention sweep runs over EVERY entry on every call, before the lookup (`:533-537`).
        // `now - seenAt` is a JS number subtraction that can go negative on a clock step; the
        // comparison is then false, so `saturating_sub` reproduces it exactly (0 is never `>`
        // the retention).
        self.seen.retain(|_, seen_at| {
            now.saturating_sub(*seen_at) <= INBOUND_MESSAGE_DEDUPE_RETENTION_MS
        });
        if self.order.len() != self.seen.len() {
            self.order.retain(|k| self.seen.contains_key(k));
        }
        if self.seen.contains_key(&key) {
            return true;
        }
        self.seen.insert(key.clone(), now);
        self.order.push_back(key);
        while self.seen.len() > INBOUND_MESSAGE_DEDUPE_MAX {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.seen.remove(&oldest);
        }
        false
    }
}

/// The three fields `latestOutboundReceipts` keeps per message (`v0.10.1 index.ts:528`,
/// written at `:1019-1023`) — deliberately NOT the whole [`MessageReceipt`], because upstream
/// drops `messageId` (it is the map key) and spreads `detail` in only when truthy.
#[derive(Clone, Debug)]
pub struct OutboundReceipt {
    /// The latest status reported for the message.
    pub status: MessageReceiptStatus,
    /// Its epoch-ms — `[JS-NUMBER]`.
    pub timestamp: serde_json::Number,
    /// The receipt's free-form detail, when it carried a non-empty one.
    pub detail: Option<String>,
}

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
    /// The project-pane launcher backend, late-bound exactly like [`Self::host_services`] and for
    /// the same reason: it is a host capability, not session data, and a headless session has none.
    /// `None` until a backend is bound; [`crate::tools::intercom`] then answers
    /// `openProjectPaneIfMissing` with [`crate::project_pane::UnavailableLauncher`] — a true
    /// `HERDR_UNAVAILABLE` — rather than ignoring the flag.
    project_pane_launcher: Mutex<Option<Arc<dyn crate::project_pane::ProjectPaneLauncher>>>,
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
    /// `seenInboundMessages` (`v0.10.1 index.ts:527`) — the inbound `(from.id, message.id)` dedupe
    /// set. Reachable in practice, not theoretically: the reconnect ladder re-registers the same
    /// session id, and ICOM-010's broker mailbox redelivers on that re-register, so a message
    /// acknowledged just before a drop can arrive a second time. Without this, it is injected twice.
    seen_inbound_messages: Mutex<SeenInboundMessages>,
    /// `latestOutboundReceipts` (`v0.10.1 index.ts:528`) — the newest receipt seen for each message
    /// THIS session sent, read back by [`Self::latest_delivery_state`].
    latest_outbound_receipts: Mutex<HashMap<String, OutboundReceipt>>,
    /// `outboxRequestIds` (`v0.12.0 index.ts:645`) — every outbox `requestId` seen in THIS runtime,
    /// the source of `duplicate_request`. Cleared by [`crate::connect::begin_runtime`]
    /// (`index.ts:1582`) and never pruned by time: the dedupe window IS the runtime, so replaying a
    /// requestId after a session restart is legal.
    /// `extensionRegistry` (`v0.12.0 index.ts:856-861`) — the `(namespace, ownerEligible)` pairs
    /// registered on this session, the source `currentExtensionCapabilities` reads. ICOM-056 lands
    /// the FRONT DOOR only: the pairs are recorded here, but the channel effects behind them (owner
    /// election, publish fan-out, the state store) are ICOM-016 and are not stubbed.
    extension_registrations: Mutex<HashMap<String, bool>>,
    outbox_request_ids: Mutex<HashSet<String>>,
    /// `pendingOutboxRequests` (`v0.12.0 index.ts:646`) — in-flight outbox requests keyed by
    /// `requestId`, each stamped with the generation it started under so
    /// [`crate::outbox::fail_pending_outbox_requests`] can settle exactly the ones a runtime change
    /// orphaned and leave a newer runtime's requests alone.
    pending_outbox_requests: Mutex<HashMap<String, crate::outbox::PendingOutboxRequest>>,
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
            project_pane_launcher: Mutex::new(None),
            has_ui: AtomicBool::new(false),
            active_tools: Mutex::new(Vec::new()),
            agent_running: AtomicBool::new(false),
            presence_metadata: Mutex::new(None),
            last_presence_identity: Mutex::new(None),
            name_poll_task: Mutex::new(None),
            connect: crate::connect::ConnectSupervisor::default(),
            seen_inbound_messages: Mutex::new(SeenInboundMessages::default()),
            latest_outbound_receipts: Mutex::new(HashMap::new()),
            extension_registrations: Mutex::new(HashMap::new()),
            outbox_request_ids: Mutex::new(HashSet::new()),
            pending_outbox_requests: Mutex::new(HashMap::new()),
            tracker: Mutex::new(ReplyTracker::new(ask_timeout_ms)),
            waiter: OutboundReplyWaiter::new(),
            config,
            ask_timeout_ms,
            cwd,
        }
    }

    /// `hasSeenInboundMessage(from, message, now)` (`v0.10.1 index.ts:532-548`) — has this session
    /// already accepted this `(from.id, message.id)` pair? Test-and-insert: a `false` answer also
    /// RECORDS the pair, so the second delivery of the same message answers `true`.
    ///
    /// The key is upstream's `` `${from.id}\0${message.id}` `` (`:538`) verbatim, NUL separator
    /// included — a separator that cannot occur in either id is what stops `("a\0b", "c")` and
    /// `("a", "b\0c")` colliding.
    #[must_use]
    pub fn has_seen_inbound_message(&self, from_id: &str, message_id: &str, now: u64) -> bool {
        let key = format!("{from_id}\0{message_id}");
        self.seen_inbound_messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .test_and_insert(key, now)
    }

    /// `emitMessageReceipt(messageId, status, detail?)` (`v0.10.1 index.ts:550-559`).
    ///
    /// Best-effort and silent on failure — upstream's body is wrapped in a bare `try {} catch {}`
    /// whose own comment is "Receipts are diagnostics; message handling should not fail when the
    /// sender disconnects" — so a disconnected client, an unbound client, or a write error must all
    /// be no-ops rather than propagate into the inbound path that called this.
    ///
    /// `...(detail ? { detail } : {})` (`:556`) is a JS TRUTHINESS test, so an EMPTY detail string
    /// omits the key rather than sending `""`. `Option::filter` reproduces that; a bare `Some("")`
    /// would put a key on the wire pi never puts there.
    pub fn emit_message_receipt(
        &self,
        message_id: &str,
        status: MessageReceiptStatus,
        detail: Option<&str>,
    ) {
        let Some(client) = self.client() else { return };
        client.send_message_receipt(MessageReceipt {
            message_id: message_id.to_string(),
            status,
            timestamp: now_ms().into(),
            detail: detail.filter(|d| !d.is_empty()).map(str::to_string),
            extra: Default::default(),
        });
    }

    /// `case "message_receipt"` (`v0.10.1 index.ts:1018-1024`) — remember the newest receipt for a
    /// message THIS session sent. Last writer wins; upstream `Map.set`s unconditionally.
    pub fn record_outbound_receipt(&self, receipt: &MessageReceipt) {
        self.latest_outbound_receipts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                receipt.message_id.clone(),
                OutboundReceipt {
                    status: receipt.status,
                    timestamp: receipt.timestamp.clone(),
                    detail: receipt.detail.clone().filter(|d| !d.is_empty()),
                },
            );
    }

    /// `latestDeliveryState(messageId, fallback)` (`v0.10.1 index.ts:570-576`) — the newest receipt
    /// status for `message_id`, or `fallback` when there is no message id or no receipt yet.
    #[must_use]
    pub fn latest_delivery_state(&self, message_id: Option<&str>, fallback: &str) -> String {
        let Some(message_id) = message_id else {
            return fallback.to_string();
        };
        self.latest_outbound_receipts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(message_id)
            .map_or_else(
                || fallback.to_string(),
                |r| r.status.wire_name().to_string(),
            )
    }

    /// `handleMessageControl(control)` (`v0.10.1 index.ts:562-569`) — a peer withdrew or replaced a
    /// message it had sent US.
    ///
    /// The `dismissPendingAsk` comes FIRST and is unconditional (`:563`), ahead of the branch: both
    /// actions retract the pending ask, and only the receipt they emit differs. Getting that order
    /// wrong would leave a superseded ask in the peer's `pending` list forever, which is the exact
    /// symptom ICOM-017 records.
    pub fn handle_message_control(&self, control: &MessageControl) {
        self.tracker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .dismiss_pending_ask(&control.message_id);
        match control.action {
            MessageControlAction::Cancel => self.emit_message_receipt(
                &control.message_id,
                MessageReceiptStatus::CancellationRequested,
                Some("message may already be injected or processed"),
            ),
            MessageControlAction::Supersede => {
                // `control.supersededBy ? \`superseded by ${…}\` : undefined` (`:568`).
                let detail = control
                    .superseded_by
                    .as_deref()
                    .map(|by| format!("superseded by {by}"));
                self.emit_message_receipt(
                    &control.message_id,
                    MessageReceiptStatus::Superseded,
                    detail.as_deref(),
                );
            }
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
        self.host_services
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Late-bind the project-pane launcher (ICOM-042 §5-A: `HerdrLauncher`). Bound from the same
    /// place `set_host_services` is, and idempotent for the same reason.
    pub fn set_project_pane_launcher(
        &self,
        launcher: Arc<dyn crate::project_pane::ProjectPaneLauncher>,
    ) {
        *self
            .project_pane_launcher
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(launcher);
    }

    /// The bound project-pane launcher, if any. `None` is not an error — it is the headless case,
    /// and the caller substitutes [`crate::project_pane::UnavailableLauncher`].
    #[must_use]
    pub fn project_pane_launcher(
        &self,
    ) -> Option<Arc<dyn crate::project_pane::ProjectPaneLauncher>> {
        self.project_pane_launcher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
        self.host_services()
            .is_none_or(|services| services.is_idle())
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
        self.active_tools
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(id, _)| id != call_id);
    }

    /// `activeTools.clear()` (`v0.10.1 index.ts:1430`, `:1452`, `:1409`) plus the `agentRunning`
    /// flag those same three sites set — the two always move together upstream.
    pub fn set_agent_running(&self, running: bool) {
        self.agent_running.store(running, Ordering::SeqCst);
        self.active_tools
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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
        let active_tool = self
            .active_tools
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .first()
            .map(|(_, n)| n.clone());
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
        let window = obj
            .get("contextWindow")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if window == 0 {
            return PresenceContext::default();
        }
        let tokens = obj
            .get("usedTokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let (pct, tokens) = if tokens == 0 {
            // pi `{ tokens: null, percent: null }` — send the CLEAR, do not omit.
            (Some(None), Some(None))
        } else {
            let pct = u64::try_from(
                (u128::from(tokens) * 100 + u128::from(window) / 2) / u128::from(window),
            )
            .unwrap_or(u64::MAX);
            (
                Some(Some(serde_json::Number::from(pct))),
                Some(Some(serde_json::Number::from(tokens))),
            )
        };
        PresenceContext {
            pct,
            tokens,
            window: Some(Some(serde_json::Number::from(window))),
        }
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
        let metadata = self
            .presence_metadata
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        *self
            .last_presence_identity
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
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
                let metadata = state
                    .presence_metadata
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
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
        let previous = self
            .name_poll_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(handle);
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    /// `clearNamePollTimer()` (`v0.10.1 index.ts:1407`, on `session_shutdown`).
    pub fn stop_name_poll(&self) {
        if let Some(task) = self
            .name_poll_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
        *self
            .last_presence_identity
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
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
        let metadata = self
            .presence_metadata
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        add(crate::connect::presence_identity_name(
            self,
            metadata.as_ref(),
        ));
        targets.contains(&to.trim().to_lowercase())
    }

    /// Publish this session's child-orchestrator metadata for [`Self::sync_presence_identity`].
    pub fn set_presence_metadata(
        &self,
        metadata: Option<crate::identity::ChildOrchestratorMetadata>,
    ) {
        *self
            .presence_metadata
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = metadata;
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
        let metadata = self
            .presence_metadata
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let identity = crate::connect::presence_identity(self, metadata.as_ref());
        // `lastPresenceName = identity.name; lastPresenceRuntimeFallbackAlias = …`
        // (`v0.10.1 index.ts:813-814`) — every sync updates the poll's baseline, so a rename that
        // reached the broker through `turn_start` or a tool call does not also fire from the poll.
        *self
            .last_presence_identity
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(identity.clone());
        let usage = self.current_context_usage();
        client.update_presence_full(
            identity.name.clone(),
            identity
                .name
                .as_ref()
                .map(|_| identity.runtime_fallback_alias),
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
    pub async fn resolve_target(
        &self,
        client: &Arc<IntercomClient>,
        name_or_id: &str,
    ) -> Result<Option<String>> {
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

        let by_id_prefix: Vec<&crate::transport::protocol::SessionInfo> = sessions
            .iter()
            .filter(|s| s.id.starts_with(name_or_id))
            .collect();
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
        self.client
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// This session's own broker-assigned id, if connected (for the self-target guard).
    #[must_use]
    /// Record one `intercom:extension-register` (`v0.12.0 index.ts:856-861`). Re-registering an
    /// existing namespace is refused, matching upstream's already-registered branch.
    pub fn record_extension_registration(&self, namespace: &str, owner_eligible: bool) -> bool {
        let mut guard = self
            .extension_registrations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if guard.contains_key(namespace) {
            return false;
        }
        guard.insert(namespace.to_string(), owner_eligible);
        true
    }

    /// `currentExtensionCapabilities` (`v0.12.0 index.ts:856-861`) — the registered namespaces.
    #[must_use]
    pub fn extension_registrations(&self) -> Vec<(String, bool)> {
        self.extension_registrations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Test-and-insert on `outboxRequestIds` (`v0.12.0 index.ts:1063`): `true` means this
    /// `requestId` was ALREADY seen in this runtime, i.e. `duplicate_request`. A `false` answer also
    /// records it, so the second emit of the same id answers `true`.
    pub fn outbox_request_seen(&self, request_id: &str) -> bool {
        !self
            .outbox_request_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request_id.to_string())
    }

    /// `outboxRequestIds.clear()` (`v0.12.0 index.ts:1582`) — the dedupe window is one runtime.
    pub fn clear_outbox_request_ids(&self) {
        self.outbox_request_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Track an in-flight request so a runtime change can settle it (`index.ts:1064`).
    pub fn track_pending_outbox(
        &self,
        request_id: String,
        pending: crate::outbox::PendingOutboxRequest,
    ) {
        self.pending_outbox_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request_id, pending);
    }

    /// Pop one in-flight request. `None` means it was already settled — the caller must then emit
    /// NOTHING, which is how `settleOutboxRequest` stays exactly-once (`index.ts:1009-1021`).
    pub fn take_pending_outbox(
        &self,
        request_id: &str,
    ) -> Option<crate::outbox::PendingOutboxRequest> {
        self.pending_outbox_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id)
    }

    /// Drain every in-flight request stamped at or before `generation`
    /// (`failPendingOutboxRequests`, `v0.12.0 index.ts:1022-1028`).
    ///
    /// `<=`, deliberately: both call sites run BEFORE their generation bump and pass the CURRENT
    /// generation, and the requests they are orphaning are stamped with exactly that value. A `<`
    /// here would drain nothing and leak every in-flight request on a runtime change. A request
    /// started under a LATER generation is left alone.
    pub fn drain_pending_outbox_upto(
        &self,
        generation: u64,
    ) -> Vec<crate::outbox::PendingOutboxRequest> {
        let mut guard = self
            .pending_outbox_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let stale: Vec<String> = guard
            .iter()
            .filter(|(_, p)| p.generation <= generation)
            .map(|(k, _)| k.clone())
            .collect();
        stale.iter().filter_map(|k| guard.remove(k)).collect()
    }

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
            .ask_and_wait_with_reply_to(
                client,
                target,
                question_id,
                text,
                attachments,
                None,
                None,
                None,
                cancel,
            )
            .await?;
        // Inline the reply's attachments into the visible body, exactly as pi does
        // (`replyText + formatAttachments(replyMessage.content.attachments)`,
        // `v0.10.1 index.ts:2168-2170`, `index.ts:1354-1357`) — never silently drop them.
        Ok(inline_reply_attachments(
            message.content.text,
            message.content.attachments.as_deref(),
        ))
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
        supersedes: Option<String>,
        retry_of: Option<String>,
        cancel: &cyrup_core::CancelToken,
    ) -> Result<crate::transport::protocol::Message> {
        // Single-slot guard (`if replyWaiter → "Already waiting for a reply"`).
        let rx = self
            .waiter
            .register(target.to_string(), question_id.clone())
            .map_err(IntercomError::Client)?;

        let send_result = client
            .send(
                target,
                SendOptions {
                    text,
                    attachments,
                    reply_to,
                    expects_reply: Some(true),
                    message_id: Some(question_id.clone()),
                    supersedes,
                    retry_of,
                    provenance: None,
                },
            )
            .await;

        let delivery_state = match send_result {
            // ICOM-054 — `deliveryState = sendResult.delivery` (`v0.13.0 index.ts:2464`), which
            // replaced the hard-coded `"socket_delivered"` ternary. The timeout message below can
            // now say `queued` when the peer was offline, instead of claiming the message reached
            // its socket.
            Ok(result) if result.delivered => result.delivery,
            Ok(result) => {
                self.waiter.clear_matching(&question_id);
                return Err(IntercomError::Client(
                    result
                        .reason
                        .unwrap_or_else(|| "ask was not delivered".to_string()),
                ));
            }
            Err(e) => {
                self.waiter.clear_matching(&question_id);
                return Err(e);
            }
        };

        let timeout = Duration::from_millis(self.ask_timeout_ms);
        tokio::select! {
            // `biased;` is REQUIRED, not a micro-optimisation. Upstream's waiter is not a race
            // between three peers: the reply path calls `replyWaiter.resolve(receivedMessage)`
            // (`v0.10.1 index.ts:922`) which runs `cleanup()` (`:611`) → `clearTimeout(timeout)`
            // (`:596`) and `removeEventListener("abort", onAbort)` (`:597`) BEFORE resolving. So a
            // reply that has landed permanently disarms both the deadline and the abort — the
            // timeout can never fire afterwards, at any load.
            //
            // An unbiased `tokio::select!` polls ready arms in RANDOM order, so once the one-shot
            // already holds the answer AND the deadline has also elapsed (routine on a loaded
            // runtime: the reply landing does not preempt this task), it picks the timeout arm ~50%
            // of the time — clearing the waiter, `cancel_ask`ing the edge broker-side, discarding a
            // delivered answer, and telling the model the peer never replied. Biased polling with
            // the reply arm first is the only ordering that expresses upstream's disarm.
            //
            // Same shape, same fix as the sibling reply-vs-countdown race in
            // `cyrup-session-svc/src/host_services.rs` (`LiveHostServices::ui_roundtrip`), which
            // already carries `biased;` with its reply arm first.
            biased;
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
            // Second under `biased;`, ahead of the deadline: upstream's `onAbort` runs
            // SYNCHRONOUSLY inside `abort()` (DOM event dispatch), while the deadline is a macrotask
            // that can only run at the next timer phase — so an abort raised while the timer is
            // already due still wins there, and `cleanup()` (`:596`) disarms the timer behind it.
            // It also keeps the two arms' messages honest: the timeout text asserts "this waiter
            // timeout is not cancellation", which is a lie to the model on a call that WAS
            // cancelled.
            () = cancel.cancelled() => {
                self.waiter.clear_matching(&question_id);
                client.cancel_ask(&question_id);
                Err(IntercomError::Client("Cancelled".to_string()))
            }
            () = tokio::time::sleep(timeout) => {
                self.waiter.clear_matching(&question_id);
                client.cancel_ask(&question_id);
                // `v0.10.1 index.ts:594` verbatim. Three facts cyrup's short form dropped, and each
                // changes what the model does next: WHICH message went unanswered (the id it would
                // `cancel` or `supersede`), the LAST KNOWN DELIVERY STATE (ICOM-017's receipt map is
                // read here and nowhere else — `receiver_received` vs `injected` vs nothing at all
                // is the difference between "the peer is thinking" and "the peer never saw it"), and
                // that the timeout is NOT a cancellation, so the peer may still act on it. Without
                // the last sentence a supervisor re-sends work a peer is already doing.
                //
                // The fallback is the SEND's own `delivery` (`v0.13.0 index.ts:2464`,
                // `latestDeliveryState(questionId, deliveryState)` at `:2452`) rather than
                // upstream's initial `"created"` (`:2408`), because this branch is only reachable
                // after the delivered arm above assigned it. ICOM-054 made it the ack's real value:
                // a message parked for an offline peer now reports `queued` here.
                let delivery_state = self
                    .latest_delivery_state(Some(&question_id), delivery_state.wire_name());
                Err(IntercomError::Client(format!(
                    "No reply from \"{target}\" for message {question_id} within {}. Last known \
                     delivery state: {delivery_state}. This waiter timeout is not cancellation; the \
                     delivered message may still be queued or actionable in the recipient session.",
                    describe_timeout(self.ask_timeout_ms)
                )))
            }
        }
    }
}

/// Inline a reply's attachments into its visible text (pi `replyText + formatAttachments(...)`,
/// `index.ts:1646-1649` (ask) and `index.ts:1354-1357` (contact_supervisor)) — attachments the
/// replying session sent back must never be silently dropped.
fn inline_reply_attachments(
    text: String,
    attachments: Option<&[crate::transport::protocol::Attachment]>,
) -> String {
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
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;

    /// ICOM-017 — `hasSeenInboundMessage` (`v0.10.1 index.ts:532-548`).
    ///
    /// Three independent properties, each of which was absent (there was no dedupe at all):
    /// the second delivery of the same `(from, id)` pair is suppressed, a DIFFERENT sender with the
    /// same message id is NOT, and an entry older than the retention window is forgotten so the
    /// same pair is accepted again.
    #[test]
    fn inbound_dedupe_suppresses_a_redelivery_and_keys_on_the_sender_too() {
        let mut seen = SeenInboundMessages::default();
        // Presence before absence: the FIRST sighting must be reported as new, or every
        // "suppressed" assertion below would hold against a function that always returns `true`.
        assert!(!seen.test_and_insert("peer-a\0m1".to_string(), 1_000));
        assert!(seen.test_and_insert("peer-a\0m1".to_string(), 1_001));
        // The NUL-separated key is `${from.id}\0${message.id}` (`:538`): the same message id from a
        // different peer is a different message.
        assert!(!seen.test_and_insert("peer-b\0m1".to_string(), 1_002));

        // `now - seenAt > INBOUND_MESSAGE_DEDUPE_RETENTION_MS` sweeps the entry (`:533-537`), so the
        // pair is accepted again rather than suppressed forever.
        let later = 1_000 + INBOUND_MESSAGE_DEDUPE_RETENTION_MS + 1;
        assert!(!seen.test_and_insert("peer-a\0m1".to_string(), later));
    }

    /// ICOM-017 — the size cap evicts the OLDEST key (`keys().next().value`, `:544-546`), which is
    /// the half a bare `HashMap` cannot express.
    ///
    /// RED if `order` is dropped and eviction picks an arbitrary entry: this asserts the specific
    /// pair that must have survived, not merely that the map shrank.
    #[test]
    fn inbound_dedupe_cap_evicts_the_oldest_insertion_not_an_arbitrary_one() {
        let mut seen = SeenInboundMessages::default();
        for i in 0..INBOUND_MESSAGE_DEDUPE_MAX {
            assert!(!seen.test_and_insert(format!("p\0m{i}"), 10_000));
        }
        assert_eq!(seen.seen.len(), INBOUND_MESSAGE_DEDUPE_MAX);
        // One past the cap: `m0` (the oldest) is evicted; the newest and the second-oldest stay.
        assert!(!seen.test_and_insert("p\0overflow".to_string(), 10_001));
        assert_eq!(seen.seen.len(), INBOUND_MESSAGE_DEDUPE_MAX);
        assert!(
            !seen.seen.contains_key("p\0m0"),
            "the oldest key must be the one evicted at the cap"
        );
        assert!(
            seen.seen.contains_key("p\0m1"),
            "the second-oldest must survive one eviction"
        );
        assert!(
            seen.seen.contains_key("p\0overflow"),
            "the newly inserted key must survive"
        );
    }

    /// ICOM-017 — `latestDeliveryState(messageId, fallback)` (`v0.10.1 index.ts:570-576`), the ONLY
    /// reader of `latestOutboundReceipts` and therefore the thing that makes recording receipts
    /// observable. It is what the ask timeout quotes.
    #[test]
    fn latest_delivery_state_falls_back_then_reports_the_recorded_status() {
        let s = SharedIntercomState::new(
            IntercomConfig::default(),
            600_000,
            std::path::PathBuf::from("/w"),
        );
        assert_eq!(s.latest_delivery_state(None, "created"), "created");
        assert_eq!(
            s.latest_delivery_state(Some("q1"), "socket_delivered"),
            "socket_delivered"
        );

        s.record_outbound_receipt(&MessageReceipt {
            message_id: "q1".to_string(),
            status: MessageReceiptStatus::ReceiverReceived,
            timestamp: 1u64.into(),
            detail: None,
            extra: Default::default(),
        });
        assert_eq!(
            s.latest_delivery_state(Some("q1"), "socket_delivered"),
            "receiver_received"
        );
        // `Map.set` is unconditional — the newest receipt wins.
        s.record_outbound_receipt(&MessageReceipt {
            message_id: "q1".to_string(),
            status: MessageReceiptStatus::Injected,
            timestamp: 2u64.into(),
            detail: None,
            extra: Default::default(),
        });
        assert_eq!(
            s.latest_delivery_state(Some("q1"), "socket_delivered"),
            "injected"
        );
        // A different message is untouched by either write.
        assert_eq!(s.latest_delivery_state(Some("q2"), "created"), "created");
    }

    /// ICOM-017 — `handleMessageControl` (`v0.10.1 index.ts:562-569`): a peer's `cancel` must
    /// retract the ask from THIS session's pending list.
    ///
    /// Before this, `message_control` was decoded and dropped (`transport/client.rs`), so a
    /// cancelled ask sat in `pending` until the ask timeout — the exact symptom ICOM-017 records.
    /// The `dismissPendingAsk` is upstream's FIRST statement and is unconditional, so the same
    /// assertion must hold for `supersede`; both are checked, because a port that put the dismissal
    /// inside the `cancel` branch would pass a cancel-only test.
    #[test]
    fn a_peer_control_frame_retracts_the_pending_ask_for_both_actions() {
        for action in [
            MessageControlAction::Cancel,
            MessageControlAction::Supersede,
        ] {
            let s = SharedIntercomState::new(
                IntercomConfig::default(),
                600_000,
                std::path::PathBuf::from("/w"),
            );
            let from = crate::transport::protocol::SessionInfo {
                endpoint_epoch: None,
                id: "peer".to_string(),
                name: None,
                runtime_fallback_alias: None,
                cwd: "/w".to_string(),
                model: "m".to_string(),
                pid: 1u32.into(),
                started_at: 0u64.into(),
                last_activity: 0u64.into(),
                status: None,
                peer_uid: None,
                trusted_local: None,
                context_pct: None,
                context_tokens: None,
                context_window: None,
                tmux_pane: None,
                extra: Default::default(),
            };
            let message = crate::transport::protocol::Message {
                id: "m-ask".to_string(),
                timestamp: 0u64.into(),
                expects_reply: Some(true),
                ..Default::default()
            };
            s.tracker
                .lock()
                .unwrap()
                .record_incoming_message(from, message, 0);
            assert_eq!(
                s.tracker.lock().unwrap().list_pending(0).len(),
                1,
                "fixture: the ask must be pending BEFORE the control frame, or its absence \
                 afterwards proves nothing"
            );

            s.handle_message_control(&MessageControl {
                message_id: "m-ask".to_string(),
                action,
                timestamp: 1u64.into(),
                superseded_by: None,
                detail: None,
                extra: Default::default(),
            });
            assert!(
                s.tracker.lock().unwrap().list_pending(0).is_empty(),
                "a {action:?} control frame must retract the pending ask"
            );
        }
    }

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
        let config = IntercomConfig {
            status: Some("reviewing".to_string()),
            ..IntercomConfig::default()
        };
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
        let s = SharedIntercomState::new(
            IntercomConfig::default(),
            600_000,
            std::path::PathBuf::from("/w"),
        );
        assert_eq!(s.current_status(), "idle");
        let blank = IntercomConfig {
            status: Some("   ".to_string()),
            ..IntercomConfig::default()
        };
        let s = SharedIntercomState::new(blank, 600_000, std::path::PathBuf::from("/w"));
        assert_eq!(s.current_status(), "idle");
    }

    /// ICOM-034 / `currentSessionTargetMatches` (`v0.10.1 index.ts:850-863`). With no live client
    /// and no host services there is nothing to match, which is the degraded default; the trimmed,
    /// lower-cased set membership is what the seams rely on.
    #[test]
    fn current_session_target_matches_is_false_without_an_identity() {
        let s = SharedIntercomState::new(
            IntercomConfig::default(),
            600_000,
            std::path::PathBuf::from("/w"),
        );
        assert!(!s.current_session_target_matches("anyone", None));
        // The resolved-id arm needs a live client id, so it cannot fire either.
        assert!(!s.current_session_target_matches("anyone", Some("some-id")));
    }

    #[test]
    fn set_and_clear_client() {
        let state = SharedIntercomState::new(
            IntercomConfig::default(),
            600_000,
            std::path::PathBuf::from("/w"),
        );
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
        assert_eq!(
            text,
            "Looks good\n\n---\nAttachment: patch.diff\n~~~diff\n+1 line\n~~~"
        );
    }

    /// No attachments ⇒ the reply text passes through unchanged (pi: `replyAttachments = ""` when
    /// `replyMessage.content.attachments?.length` is falsy).
    #[test]
    fn inline_reply_attachments_passes_through_when_none() {
        assert_eq!(
            inline_reply_attachments("no attachments here".to_string(), None),
            "no attachments here"
        );
        assert_eq!(
            inline_reply_attachments("empty vec".to_string(), Some(&[])),
            "empty vec"
        );
    }

    /// The ask/reply race in [`SharedIntercomState::ask_and_wait_with_reply_to`], driven into the
    /// exact state the unbiased `select!` decided by coin flip: the reply is ALREADY sitting in the
    /// one-shot **and** the deadline has ALREADY elapsed when the future is first polled.
    ///
    /// Upstream cannot reach this state at all — `replyWaiter.resolve` (`v0.10.1 index.ts:922`)
    /// runs `cleanup()` → `clearTimeout` (`:596`) before it resolves, so a landed reply permanently
    /// disarms the deadline. `tokio::select!` without `biased;` polls ready arms in RANDOM order,
    /// so the timeout arm won ~50% of the time — clearing the waiter, cancelling the ask edge
    /// broker-side, discarding the peer's delivered answer, and reporting "No reply" to the model.
    ///
    /// Determinism, not luck: a gated fake broker holds the `send` ack until the test has put the
    /// reply in the slot, and `ask_timeout_ms = 0` makes `sleep(0)` ready on the first poll. So
    /// EVERY iteration reaches the `select!` with both arms already ready.
    ///
    /// Pre-fix arithmetic: `select!` picks a uniform start index over its three branches and takes
    /// the first READY one in that rotation. With the old order `(reply, timeout, cancel)` and only
    /// `reply`/`timeout` ready, start 0 → reply, start 1 → timeout, start 2 → reply, i.e. the answer
    /// is dropped 1 time in 3. Over 40 iterations the old code survives with probability
    /// `(2/3)^40 ≈ 9·10⁻⁸`. After the fix `biased;` makes the reply arm unconditional, so all 40
    /// must return the peer's message.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_already_delivered_reply_beats_an_already_elapsed_ask_deadline() {
        use crate::transport::framing::{FrameReader, encode_json};
        use crate::transport::protocol::{
            BrokerMessage, MessageContent, SessionInfo, SessionRegistration,
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        for attempt in 0..40 {
            let _dir = tempfile::tempdir().unwrap();
            // Bound through the broker's own listen abstraction so this proof runs on the
            // named-pipe arm too, rather than pinning the race to POSIX.
            #[cfg(unix)]
            let target = crate::transport::target::BrokerConnectTarget::Socket(
                _dir.path().join("broker.sock"),
            );
            #[cfg(windows)]
            let target = crate::transport::target::BrokerConnectTarget::Socket(
                std::path::PathBuf::from(format!(
                    r"\\.\pipe\cyrup-intercom-askrace-{}-{attempt}",
                    std::process::id()
                )),
            );
            let mut listener = crate::broker::listener::BrokerListener::bind(&target)
                .await
                .unwrap();

            // The broker saw the `send` frame; the test may now fill the reply slot.
            let send_seen = Arc::new(tokio::sync::Notify::new());
            // The test filled the slot; the broker may now release the `delivered` ack.
            let ack_gate = Arc::new(tokio::sync::Notify::new());
            let (broker_send_seen, broker_ack_gate) = (send_seen.clone(), ack_gate.clone());

            let broker = tokio::spawn(async move {
                let mut stream = listener.accept().await.unwrap();
                let mut reader = FrameReader::new();
                let mut buf = vec![0u8; 4096];
                let mut registered = false;
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    for payload in reader.push(&buf[..n]).unwrap() {
                        let frame: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                        match frame["type"].as_str() {
                            Some("register") if !registered => {
                                registered = true;
                                let out = encode_json(&BrokerMessage::Registered {
                                    session_id: "self".to_string(),
                                    features: None,
                                })
                                .unwrap();
                                stream.write_all(&out).await.unwrap();
                            }
                            Some("send") => {
                                let id = frame["message"]["id"].as_str().unwrap().to_string();
                                // Hand control to the test so the reply lands in the one-shot
                                // BEFORE `client.send()` resolves — i.e. before the `select!` is
                                // ever polled.
                                broker_send_seen.notify_one();
                                broker_ack_gate.notified().await;
                                let out = encode_json(&BrokerMessage::delivered_bare(id)).unwrap();
                                stream.write_all(&out).await.unwrap();
                            }
                            _ => {}
                        }
                    }
                }
            });

            let registration = SessionRegistration {
                runtime_fallback_alias: None,
                name: None,
                cwd: "/w".to_string(),
                model: "m".to_string(),
                pid: 1u32.into(),
                started_at: now_ms().into(),
                last_activity: now_ms().into(),
                status: None,
                tmux_pane: None,
                extra: Default::default(),
            };
            let client = Arc::new(
                IntercomClient::connect_target(&target, registration, Some("self".into()))
                    .await
                    .unwrap(),
            );

            // `ask_timeout_ms = 0` ⇒ `tokio::time::sleep(Duration::ZERO)` is ready on the very
            // first poll, exactly like a deadline that elapsed while the task was descheduled.
            let state = Arc::new(SharedIntercomState::new(
                IntercomConfig::default(),
                0,
                "/w".into(),
            ));

            let ask = tokio::spawn({
                let (state, client) = (state.clone(), client.clone());
                async move {
                    state
                        .ask_and_wait_with_reply_to(
                            &client,
                            "peer",
                            "q1".to_string(),
                            "are you there?".to_string(),
                            None,
                            None,
                            None,
                            None,
                            &cyrup_core::CancelToken::new(),
                        )
                        .await
                }
            });

            send_seen.notified().await;
            let reply = crate::transport::protocol::Message {
                id: "r1".to_string(),
                timestamp: now_ms().into(),
                reply_to: Some("q1".to_string()),
                content: MessageContent {
                    text: "yes".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            let peer = SessionInfo {
                endpoint_epoch: None,
                id: "peer".to_string(),
                name: Some("peer".to_string()),
                runtime_fallback_alias: None,
                cwd: "/w".to_string(),
                model: "m".to_string(),
                pid: 2u32.into(),
                started_at: 0u64.into(),
                last_activity: 0u64.into(),
                status: None,
                peer_uid: None,
                trusted_local: None,
                context_pct: None,
                context_tokens: None,
                context_window: None,
                tmux_pane: None,
                extra: Default::default(),
            };
            assert!(
                state.waiter.try_deliver(&peer, &reply),
                "the waiter slot must be armed"
            );
            ack_gate.notify_one();

            let outcome = ask.await.unwrap();
            let message = outcome.unwrap_or_else(|e| {
                panic!(
                    "attempt {attempt}: a reply already in the slot lost to an already-elapsed \
                     deadline — the answer was dropped and the ask edge cancelled: {e}"
                )
            });
            assert_eq!(message.id, "r1");
            assert_eq!(message.content.text, "yes");

            drop(client);
            broker.abort();
        }
    }
}
