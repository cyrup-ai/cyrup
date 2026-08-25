//! The CHILD-side watchdog registration — a 1:1 port of
//! `pi-subagents/src/watchdog/register-child.ts` (117 lines @v0.43.0).
//!
//! A spawned subagent runs the SAME [`MainWatchdogRuntime`] the orchestrator does, differing in
//! exactly three ways (`register-child.ts:74-84`):
//!
//! 1. **The config is fixed, not resolved.** `resolveConfig` is replaced by a closure returning the
//!    already-decoded [`ChildWatchdogConfig`] widened to a full `ResolvedWatchdogConfig`
//!    (`childResolvedConfig`, `:15-37`). A child never reads `settings.json` — its policy travelled
//!    from the parent in [`super::child_status::CHILD_WATCHDOG_CONFIG_ENV`], so a child cannot disagree with the parent
//!    about whether it is being watched.
//! 2. **A displayed warning is re-attributed.** `childWarningDetails` (`:39-46`) rewrites `source`
//!    to `child` (unless it is `lsp`, which stays `lsp`) and stamps the `agent`/`runId`, so the
//!    parent can tell whose watchdog spoke.
//! 3. **There is no `sendUserMessage`.** A child never auto-follows itself; the parent decides.
//!    `queueAutoFollowIfNeeded` returns early on the missing sink (`runtime.ts:666`), which is why
//!    the config's `autoFollow` block is still populated (`:26-30`) — it travels for the PARENT's
//!    benefit, reported through the status events below.
//!
//! ## The status channel
//!
//! `writeStatus` (`:48-54`) writes one NDJSON `subagent.watchdog.status` line per phase transition
//! to the child's stdout — the same stream the parent already reads run events from — so the
//! parent's `execution.ts` tail timer knows whether a settled child still has a watchdog review in
//! flight. It is advisory: a write failure is swallowed (`:51-53`), because losing a status line
//! must never fail the run.
//!
//! `emitStatus` is called from four places (`:92,104,107-109,114`) and the `seq` counter is
//! strictly increasing per child, which is what lets the parent's `acceptChildWatchdogEvent`
//! (`child-status.ts:180-201`) discard an out-of-order line.
//!
//! ## Registration gate
//!
//! `registerChildWatchdog(pi, raw)` returns `undefined` when the env carries no config, or carries
//! `enabled: false` (`:57-58`). That is the common case: every subagent process runs this, and only
//! the ones the parent explicitly armed install anything at all.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::Value;

use super::child_status::{
    decode_child_watchdog_config, ChildWatchdogConfig, ChildWatchdogPhase, ChildWatchdogStatusEvent,
    CHILD_WATCHDOG_STATUS_EVENT,
};
use super::runtime::{MainWatchdogRuntime, MainWatchdogRuntimeOptions, WatchdogReview};
use super::settings::default_watchdog_config;
use super::types::{
    ResolvedWatchdogConfig, WatchdogAutoFollowConfig, WatchdogEndpointConfig, WatchdogRuntimeStatus,
    WatchdogSettingsResult, WatchdogSettingsScope, WatchdogSettingsSource, WatchdogWarningDetails,
    WatchdogWarningSource, SUBAGENT_WATCHDOG_WARNING_TYPE,
};
use super::warning_format::create_watchdog_warning_message_from_details;

/// `childResolvedConfig(config)` (`register-child.ts:15-37`) — widen the flat env config into the
/// full resolved shape the runtime reads.
///
/// Note what it does NOT copy: `scope`, `cadence`, `guidance`, `asyncCompletion`,
/// `severityThreshold`, `delivery` and the rest keep their `DEFAULT_WATCHDOG_CONFIG` values, so a
/// child reviews on the boundary only and at the default `concern` threshold regardless of how the
/// parent's own session is configured.
#[must_use]
pub fn child_resolved_config(config: &ChildWatchdogConfig) -> ResolvedWatchdogConfig {
    let defaults = default_watchdog_config();
    ResolvedWatchdogConfig {
        enabled: true,
        agent_end_timeout_ms: config.agent_end_timeout_ms,
        max_warnings: config.max_warnings,
        main: WatchdogEndpointConfig {
            enabled: true,
            model: config.model.clone(),
            thinking: config.thinking.clone(),
        },
        auto_follow: WatchdogAutoFollowConfig {
            blockers: config.auto_follow_blockers,
            max_attempts: config.auto_follow_max_attempts,
            stalemate_repeats: config.stalemate_repeats,
        },
        children: super::types::WatchdogChildrenConfig {
            watchdog_tail_timeout_ms: config.watchdog_tail_timeout_ms,
            ..defaults.children
        },
        lsp: config.lsp.clone(),
        ..defaults
    }
}

/// `childWarningDetails(details, config)` (`register-child.ts:39-46`).
#[must_use]
pub fn child_warning_details(
    details: &WatchdogWarningDetails,
    config: &ChildWatchdogConfig,
) -> WatchdogWarningDetails {
    WatchdogWarningDetails {
        source: if details.source == WatchdogWarningSource::Lsp {
            WatchdogWarningSource::Lsp
        } else {
            WatchdogWarningSource::Child
        },
        agent: config.agent.clone().or_else(|| details.agent.clone()),
        run_id: config.run_id.clone().or_else(|| details.run_id.clone()),
        ..details.clone()
    }
}

/// Where a child's advisory status line goes. Production is stdout (`writeStatus`, `:48-54`); a test
/// substitutes a collector.
pub type ChildWatchdogStatusSink = Arc<dyn Fn(&ChildWatchdogStatusEvent) + Send + Sync>;

/// `writeStatus(event)` (`register-child.ts:48-54`) — one NDJSON line on stdout, failures swallowed.
#[must_use]
pub fn stdout_status_sink() -> ChildWatchdogStatusSink {
    Arc::new(|event: &ChildWatchdogStatusEvent| {
        use std::io::Write as _;
        let Ok(line) = serde_json::to_string(event) else {
            return;
        };
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        // "Child watchdog status is advisory; stdout failures are handled by the child process
        // itself." (`:52`).
        let _ = writeln!(handle, "{line}");
        let _ = handle.flush();
    })
}

/// The child watchdog: the runtime plus the status emitter bound to one child's identity.
///
/// Held by [`crate::prompt_runtime::SubagentPromptRuntime`], which drives the same four lifecycle
/// hooks upstream's `onRuntimeEvent` registrations do (`register-child.ts:89-115`).
pub struct ChildWatchdog {
    runtime: Arc<MainWatchdogRuntime>,
    config: ChildWatchdogConfig,
    sink: ChildWatchdogStatusSink,
    seq: AtomicU64,
}

impl std::fmt::Debug for ChildWatchdog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildWatchdog")
            .field("agent", &self.config.agent)
            .field("run_id", &self.config.run_id)
            .field("seq", &self.seq.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl ChildWatchdog {
    /// The runtime, for the caller that drives its lifecycle hooks.
    #[must_use]
    pub fn runtime(&self) -> &Arc<MainWatchdogRuntime> {
        &self.runtime
    }

    /// The decoded config this child was armed with.
    #[must_use]
    pub fn config(&self) -> &ChildWatchdogConfig {
        &self.config
    }

    /// `emitStatus(phase, followUpPending, reason)` (`register-child.ts:61-73`).
    pub fn emit_status(
        &self,
        phase: ChildWatchdogPhase,
        follow_up_pending: bool,
        reason: Option<&str>,
    ) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let event = ChildWatchdogStatusEvent {
            event_type: CHILD_WATCHDOG_STATUS_EVENT.to_string(),
            run_id: self.config.run_id.clone(),
            agent: self.config.agent.clone(),
            // `:66` — the index is emitted under BOTH spellings, so a parent reading either finds it.
            child_index: self.config.child_index,
            step_index: self.config.child_index,
            seq,
            phase,
            ts: crate::time::now_epoch_millis(),
            follow_up_pending,
            reason: reason.map(str::to_string),
        };
        (self.sink)(&event);
    }

    /// `session_start` (`register-child.ts:89-93`).
    pub fn handle_session_start(&self, cwd: &Path) {
        self.runtime.bind_session(cwd);
        self.emit_status(ChildWatchdogPhase::Idle, false, None);
    }

    /// `before_agent_start` (`register-child.ts:94-97`).
    pub fn handle_before_agent_start(&self, event: &Value, cwd: &Path) {
        self.runtime.handle_before_agent_start(event, cwd);
    }

    /// `turn_end` (`register-child.ts:98-101`).
    pub fn handle_turn_end(&self, event: &Value, cwd: &Path) {
        self.runtime.handle_turn_end(event, cwd);
    }

    /// `agent_end` (`register-child.ts:102-110`) — `reviewing` before, then the phase the runtime
    /// settled into: `failed` with its error, `stale` with the fixed "review stale" reason, else
    /// `idle`.
    pub async fn handle_agent_end(&self, cwd: &Path) {
        self.emit_status(ChildWatchdogPhase::Reviewing, false, None);
        self.runtime.handle_agent_end(cwd).await;
        let snapshot = self.runtime.get_snapshot(Some(cwd));
        match snapshot.status {
            WatchdogRuntimeStatus::Failed => self.emit_status(
                ChildWatchdogPhase::Failed,
                false,
                snapshot.last_error.as_deref(),
            ),
            WatchdogRuntimeStatus::Stale => {
                self.emit_status(ChildWatchdogPhase::Stale, false, Some("review stale"));
            }
            _ => self.emit_status(ChildWatchdogPhase::Idle, false, None),
        }
    }

    /// `session_shutdown` (`register-child.ts:111-115`).
    pub fn handle_session_shutdown(&self) {
        self.runtime.dispose();
        self.emit_status(ChildWatchdogPhase::Idle, false, None);
    }
}

/// `registerChildWatchdog(pi, rawConfig)` (`register-child.ts:56-117`).
///
/// `None` when the env carries nothing, carries `enabled: false`, or carries something that does not
/// decode — upstream throws on the last of those from inside `decodeChildWatchdogConfig`, which
/// aborts the whole child registration; this port declines to arm the watchdog instead, because a
/// cyrup child's registration also installs the steering inbox, the tool budget and the structured
/// output tool, none of which should die with a malformed watchdog env value.
///
/// `services` resolves the message-delivery capability LATE, exactly as the main role's does.
#[must_use]
pub fn register_child_watchdog(
    raw_config: Option<&str>,
    cwd: &Path,
    services: super::register_main::WatchdogServicesFn,
    review: Option<Arc<dyn WatchdogReview>>,
    sink: ChildWatchdogStatusSink,
) -> Option<Arc<ChildWatchdog>> {
    let child_config = decode_child_watchdog_config(raw_config).ok().flatten()?;
    if !child_config.enabled {
        return None;
    }
    let resolved = child_resolved_config(&child_config);
    let resolver_config = resolved.clone();
    let display_config = child_config.clone();
    let review_connected = review.is_some();
    let runtime = Arc::new(MainWatchdogRuntime::new(MainWatchdogRuntimeOptions {
        cwd: Some(cwd.to_path_buf()),
        // `:76` — a fixed, always-ok result with a single `session` source.
        resolve_config: Some(Arc::new(move |_cwd: &Path, _session: Option<&Value>| {
            WatchdogSettingsResult {
                ok: true,
                config: resolver_config.clone(),
                errors: Vec::new(),
                sources: vec![WatchdogSettingsSource {
                    scope: WatchdogSettingsScope::Session,
                    path: None,
                    exists: true,
                }],
            }
        })),
        review,
        // `:78`.
        review_description: review_connected.then(|| "child model review".to_string()),
        display_warning: Some(Arc::new(move |details, _delivery| {
            let Some(services) = services() else {
                return;
            };
            let child_details = child_warning_details(details, &display_config);
            let message = create_watchdog_warning_message_from_details(&child_details, true);
            let _ = services.inject_message(
                &message.content,
                Some(SUBAGENT_WATCHDOG_WARNING_TYPE),
                message.display,
                false,
            );
        })),
        // `:80-83` — no `sendUserMessage`: a child never auto-follows itself.
        send_user_message: None,
        review_changes_only: true,
        lsp_diagnostics: None,
        repo_change_signature: None,
    }));
    Some(Arc::new(ChildWatchdog {
        runtime,
        config: child_config,
        sink,
        seq: AtomicU64::new(0),
    }))
}

// A `register_child_watchdog_from_env(cwd, services)` convenience wrapper lived here and was
// deleted: it had no caller anywhere, and wiring one would have been a regression rather than a
// fix. It read the env itself and then passed `review: None`, which lands the child on
// `InertWatchdogReview` — a runtime that resolves no model, calls nothing and reports every
// boundary clean. Upstream has no review-less entry point: `registerChildWatchdog(pi, rawConfig =
// process.env[CHILD_WATCHDOG_CONFIG_ENV])` (`register-child.ts:56`) unconditionally builds
// `review: createMainWatchdogReview(() => currentContext, …)` at `:77`, so an armed child is always
// watched by a real review. cyrup's single equivalent entry point is
// [`register_child_watchdog`] as called from `crate::prompt_runtime` (`prompt_runtime.rs:1701`),
// which reads the same env var at `:1687-1688`, builds the review at `:1698-1700`, and passes
// [`stdout_status_sink`] — the port of `:77` and `:50`, in one place.

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use super::super::types::{
        ThinkingSetting, WatchdogCategory, WatchdogLspConfig, WatchdogSeverity,
        WatchdogWarningState,
    };
    use std::sync::Mutex;

    fn config() -> ChildWatchdogConfig {
        ChildWatchdogConfig {
            enabled: true,
            run_id: Some("run-1".to_string()),
            agent: Some("reviewer".to_string()),
            child_index: Some(2),
            watchdog_tail_timeout_ms: 90_000,
            agent_end_timeout_ms: 15_000,
            max_warnings: Some(4),
            model: Some("anthropic/opus".to_string()),
            thinking: Some(ThinkingSetting::Level("high".to_string())),
            lsp: WatchdogLspConfig {
                enabled: false,
                timeout_ms: 1_000,
                max_files: 5,
                max_diagnostics: 7,
            },
            auto_follow_blockers: false,
            auto_follow_max_attempts: Some(2),
            stalemate_repeats: 5,
        }
    }

    fn collecting_sink() -> (ChildWatchdogStatusSink, Arc<Mutex<Vec<ChildWatchdogStatusEvent>>>) {
        let events: Arc<Mutex<Vec<ChildWatchdogStatusEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        (
            Arc::new(move |event: &ChildWatchdogStatusEvent| {
                sink_events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(event.clone());
            }),
            events,
        )
    }

    #[test]
    fn the_child_config_widens_into_an_always_enabled_resolved_config() {
        let resolved = child_resolved_config(&config());
        assert!(resolved.enabled);
        assert!(resolved.main.enabled);
        assert_eq!(resolved.main.model.as_deref(), Some("anthropic/opus"));
        assert_eq!(
            resolved.main.thinking,
            Some(ThinkingSetting::Level("high".to_string()))
        );
        assert_eq!(resolved.agent_end_timeout_ms, 15_000);
        assert_eq!(resolved.max_warnings, Some(4));
        assert!(!resolved.auto_follow.blockers);
        assert_eq!(resolved.auto_follow.max_attempts, Some(2));
        assert_eq!(resolved.auto_follow.stalemate_repeats, 5);
        assert_eq!(resolved.children.watchdog_tail_timeout_ms, 90_000);
        assert!(!resolved.lsp.enabled);
        assert_eq!(resolved.lsp.max_diagnostics, 7);
        // Everything else keeps the defaults — the child does NOT inherit the parent's cadence,
        // scope or threshold.
        let defaults = default_watchdog_config();
        assert_eq!(resolved.cadence, defaults.cadence);
        assert_eq!(resolved.scope, defaults.scope);
        assert_eq!(resolved.severity_threshold, defaults.severity_threshold);
        assert_eq!(resolved.guidance, defaults.guidance);
        assert_eq!(resolved.async_completion, defaults.async_completion);
        assert_eq!(resolved.children.enabled, defaults.children.enabled);
    }

    #[test]
    fn a_child_warning_is_reattributed_to_the_child_and_stamped() {
        let details = WatchdogWarningDetails {
            severity: WatchdogSeverity::Concern,
            summary: "s".into(),
            evidence: "e".into(),
            recommended_action: "r".into(),
            category: WatchdogCategory::Other,
            source: WatchdogWarningSource::Main,
            confidence: None,
            agent: None,
            run_id: None,
            stale: None,
            auto_follow_attempt: None,
            state: Some(WatchdogWarningState::Displayed),
            identity: None,
            displayed_at: None,
            error: None,
            stalemate_repeats: None,
        };
        let child = child_warning_details(&details, &config());
        assert_eq!(child.source, WatchdogWarningSource::Child);
        assert_eq!(child.agent.as_deref(), Some("reviewer"));
        assert_eq!(child.run_id.as_deref(), Some("run-1"));
    }

    #[test]
    fn an_lsp_sourced_warning_keeps_its_lsp_attribution() {
        let mut details = WatchdogWarningDetails {
            severity: WatchdogSeverity::Blocker,
            summary: "s".into(),
            evidence: "e".into(),
            recommended_action: "r".into(),
            category: WatchdogCategory::Correctness,
            source: WatchdogWarningSource::Lsp,
            confidence: None,
            agent: None,
            run_id: None,
            stale: None,
            auto_follow_attempt: None,
            state: None,
            identity: None,
            displayed_at: None,
            error: None,
            stalemate_repeats: None,
        };
        details.source = WatchdogWarningSource::Lsp;
        let child = child_warning_details(&details, &config());
        assert_eq!(child.source, WatchdogWarningSource::Lsp);
    }

    #[test]
    fn no_env_config_installs_nothing() {
        assert!(register_child_watchdog(
            None,
            Path::new("/tmp"),
            Arc::new(|| None),
            None,
            collecting_sink().0
        )
        .is_none());
        assert!(register_child_watchdog(
            Some(""),
            Path::new("/tmp"),
            Arc::new(|| None),
            None,
            collecting_sink().0
        )
        .is_none());
    }

    #[test]
    fn an_explicitly_disabled_config_installs_nothing() {
        let raw = serde_json::json!({ "enabled": false }).to_string();
        assert!(register_child_watchdog(
            Some(&raw),
            Path::new("/tmp"),
            Arc::new(|| None),
            None,
            collecting_sink().0
        )
        .is_none());
    }

    #[test]
    fn a_malformed_config_declines_rather_than_failing_the_child() {
        let raw = "{ not json";
        assert!(register_child_watchdog(
            Some(raw),
            Path::new("/tmp"),
            Arc::new(|| None),
            None,
            collecting_sink().0
        )
        .is_none());
    }

    #[test]
    fn an_armed_child_runs_an_always_enabled_runtime() {
        let raw = serde_json::to_string(&config()).expect("encode");
        let (sink, _events) = collecting_sink();
        let child = register_child_watchdog(
            Some(&raw),
            Path::new("/tmp"),
            Arc::new(|| None),
            None,
            sink,
        )
        .expect("armed");
        let snapshot = child.runtime().get_snapshot(None);
        assert!(snapshot.enabled);
        assert_eq!(snapshot.config.agent_end_timeout_ms, 15_000);
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].scope, WatchdogSettingsScope::Session);
        assert_eq!(
            snapshot.review_trigger,
            super::super::runtime::WatchdogReviewTrigger::RepoEdits
        );
    }

    #[tokio::test]
    async fn the_status_events_are_strictly_ordered_across_the_lifecycle() {
        let raw = serde_json::to_string(&config()).expect("encode");
        let (sink, events) = collecting_sink();
        let child = register_child_watchdog(
            Some(&raw),
            Path::new("/tmp"),
            Arc::new(|| None),
            None,
            sink,
        )
        .expect("armed");

        child.handle_session_start(Path::new("/tmp"));
        child.handle_agent_end(Path::new("/tmp")).await;
        child.handle_session_shutdown();

        let events = events.lock().unwrap();
        let phases: Vec<ChildWatchdogPhase> = events.iter().map(|e| e.phase).collect();
        assert_eq!(
            phases,
            vec![
                ChildWatchdogPhase::Idle,
                ChildWatchdogPhase::Reviewing,
                ChildWatchdogPhase::Idle,
                ChildWatchdogPhase::Idle,
            ]
        );
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4], "seq is strictly increasing from 1");
        for event in events.iter() {
            assert_eq!(event.event_type, CHILD_WATCHDOG_STATUS_EVENT);
            assert_eq!(event.run_id.as_deref(), Some("run-1"));
            assert_eq!(event.agent.as_deref(), Some("reviewer"));
            assert_eq!(event.child_index, Some(2));
            assert_eq!(event.step_index, Some(2), "both index spellings are emitted");
            assert!(!event.follow_up_pending);
        }
    }

    #[test]
    fn the_status_event_serializes_under_upstreams_type_key() {
        let (sink, events) = collecting_sink();
        let child = ChildWatchdog {
            runtime: Arc::new(MainWatchdogRuntime::default()),
            config: config(),
            sink,
            seq: AtomicU64::new(0),
        };
        child.emit_status(ChildWatchdogPhase::Failed, true, Some("boom"));
        let event = events.lock().unwrap()[0].clone();
        let json = serde_json::to_value(&event).expect("json");
        assert_eq!(json["type"], serde_json::json!(CHILD_WATCHDOG_STATUS_EVENT));
        assert_eq!(json["phase"], serde_json::json!("failed"));
        assert_eq!(json["followUpPending"], serde_json::json!(true));
        assert_eq!(json["reason"], serde_json::json!("boom"));
        assert_eq!(json["stepIndex"], serde_json::json!(2));
    }
}
