//! The human-prompt seam (port of pi `permission-dialog.ts:1-159` + `promptPermission`). A permission
//! `ask` that must reach a human goes through an [`AskChannel`]. Two backends:
//!
//! - [`LocalAskChannel`] (P-1, now live): calls `HostServices::select` (multi-line compacted title,
//!   the 4 pi options, optional timeout) then, on "Reject with Reason", `HostServices::input`
//!   (`permission-dialog.ts:114-159`). Reached from native code through the late-bound
//!   `Arc<dyn HostServices>` slot (`NativeExtension::set_host_services`, reconciliation §2 item 1).
//! - [`NoOpAskChannel`]: the fail-closed fallback for a headless/no-UI context — an `ask` FAIL-CLOSES
//!   to `Block` (never silently allows, never hangs), mirroring pi's `confirmPermission` headless
//!   `{approved:false}` (`index.ts:1506-1513`) and pi's `canResolveAskPermissionRequest` block
//!   (`index.ts:2452-2467`).
//! - [`ForwardingAskChannel`] (P-4, now live): the child→parent ask-forwarding channel. When an
//!   ask-tier decision fires inside a subagent CHILD (headless, no local human), it writes a
//!   nonce-bound REQUEST into the PARENT session's filesystem spool (addressed by the
//!   `CYRUP_SUBAGENT_PARENT_SESSION` anchor) and BLOCKS on the bound RESPONSE up to the forwarding
//!   timeout — a faithful port of pi's `confirmPermission` subagent branch → `waitForForwarded
//!   PermissionApproval` (`index.ts:1514-1518,1255-1355`). The transport itself lives in
//!   [`crate::forwarding`]; this channel is the thin `AskChannel` adapter the gate installs on a child.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use cyrup_ext::{DialogOptions, HostServices};

/// pi `permission-dialog.ts:1` — SIX decision states. The dialog emits `Once`/`Always`/`Reject`;
/// other paths (yolo auto-approve, forwarding) emit the rest. The `serde` `snake_case` rename
/// reproduces pi's EXACT on-the-wire strings — `"approved" | "denied" | "denied_with_reason" |
/// "once" | "always" | "reject"` (`permission-dialog.ts:1`, `isPermissionDecisionState` `index.ts`)
/// — so the child→parent forwarding response file (`forwarding.rs`, pi `index.ts:1483-1492`) is a
/// byte-faithful port of pi's `ForwardedPermissionResponse.state` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecisionState {
    Approved,
    Denied,
    DeniedWithReason,
    Once,
    Always,
    Reject,
}

/// pi `permission-dialog.ts:3-7` — the resolved prompt decision.
#[derive(Debug, Clone)]
pub struct PermissionPromptDecision {
    pub approved: bool,
    pub state: PermissionDecisionState,
    pub denial_reason: Option<String>,
}

/// Options for a prompt (pi `requestPermissionDecisionFromUi` opts): an optional timeout after which
/// the dialog auto-rejects. Phase 0 carries it for shape parity; `NoOpAskChannel` ignores it.
#[derive(Debug, Clone, Default)]
pub struct PromptOpts {
    pub timeout: Option<std::time::Duration>,
}

/// The outcome of an [`AskChannel::confirm`] round-trip.
#[derive(Debug, Clone)]
pub enum AskOutcome {
    /// A human (or yolo/forwarding) produced a decision (allow-once / allow-always / reject).
    Decided(PermissionPromptDecision),
    /// No live channel is reachable (headless / no UI) — the gate fail-closes to `Block`. The live
    /// [`LocalAskChannel`] never yields this (it always resolves to a decision, rejecting on
    /// timeout/ESC); only [`NoOpAskChannel`] does.
    NoLiveChannel,
}

/// The single async human-prompt seam. Two backends: `NoOpAskChannel` (Phase 0) and the future
/// `LocalAskChannel` (P-1).
#[async_trait]
pub trait AskChannel: Send + Sync {
    async fn confirm(&self, title: &str, message: &str, opts: PromptOpts) -> AskOutcome;
}

/// The fail-closed Phase-0 channel: always [`AskOutcome::NoLiveChannel`]. Never panics, never
/// allows.
#[derive(Debug, Default)]
pub struct NoOpAskChannel;

#[async_trait]
impl AskChannel for NoOpAskChannel {
    async fn confirm(&self, _title: &str, _message: &str, _opts: PromptOpts) -> AskOutcome {
        AskOutcome::NoLiveChannel
    }
}

// ---------------------------------------------------------------- the live in-session dialog (P-1)

/// pi `permission-dialog.ts:24-28` — the four options, in pi's exact order.
const APPROVE_ONCE_OPTION: &str = "Allow Once";
const APPROVE_ALWAYS_OPTION: &str = "Allow Always";
const REJECT_OPTION: &str = "Reject";
const REJECT_WITH_REASON_OPTION: &str = "Reject with Reason";
/// pi `permission-dialog.ts:34-35`.
const PERMISSION_DIALOG_MAX_VISIBLE_LINES: usize = 32;
const PERMISSION_DIALOG_MAX_VISIBLE_CHARACTERS: usize = 2_200;

/// The live in-session human dialog (pi `requestPermissionDecisionFromUi`,
/// `permission-dialog.ts:114-159`): `select(compact("{title}\n{message}"), [4 options], {timeout?})`,
/// then on "Reject with Reason" a second `input(...)`. Maps to `HostServices::select` +
/// `HostServices::input` (NOT `confirm` — port doc §7.3). `HostServices::{select,input}` are the SYNC
/// blocking bridges (`cyrup-session-svc::LiveHostServices::ui_roundtrip` → `block_in_place`); they run
/// on the dispatch worker thread while the caller (the permission gate) holds a P-3
/// [`cyrup_ext::HumanWaitGuard`] so the dispatcher's invocation budget is suspended for the human
/// latency instead of failing OPEN. `REJECT_OPTION`, ESC/`None`, and a timeout all resolve to `reject`
/// (`permission-dialog.ts:155-158`), so the gate always fail-CLOSES on anything but an explicit allow.
pub struct LocalAskChannel {
    services: Arc<dyn HostServices>,
}

impl LocalAskChannel {
    #[must_use]
    pub fn new(services: Arc<dyn HostServices>) -> Self {
        Self { services }
    }
}

#[async_trait]
impl AskChannel for LocalAskChannel {
    async fn confirm(&self, title: &str, message: &str, opts: PromptOpts) -> AskOutcome {
        let prompt = compact_permission_prompt_for_select(&format!("{title}\n{message}"));
        let options = serde_json::json!([
            APPROVE_ONCE_OPTION,
            APPROVE_ALWAYS_OPTION,
            REJECT_OPTION,
            REJECT_WITH_REASON_OPTION,
        ]);
        // pi passes `{timeout}` only when a positive, finite timeout is set (`permission-dialog.ts:122`);
        // otherwise the select blocks until the human answers.
        let dialog_opts = DialogOptions {
            timeout_ms: opts.timeout.and_then(|d| u64::try_from(d.as_millis()).ok()).filter(|ms| *ms > 0),
            signal_id: None,
        };
        let selected = self.services.select(&prompt, &options, &dialog_opts);

        let decision = match selected.as_deref() {
            Some(APPROVE_ONCE_OPTION) => PermissionPromptDecision {
                approved: true,
                state: PermissionDecisionState::Once,
                denial_reason: None,
            },
            Some(APPROVE_ALWAYS_OPTION) => PermissionPromptDecision {
                approved: true,
                state: PermissionDecisionState::Always,
                denial_reason: None,
            },
            Some(REJECT_WITH_REASON_OPTION) => {
                // pi `permission-dialog.ts:141-153`: a second `input` for the (optional) reason.
                let reason = self
                    .services
                    .input(
                        &format!("{title}\nShare why this request was denied (optional)."),
                        Some("Reason shown back to the agent"),
                        &DialogOptions::default(),
                    )
                    .map(|r| r.trim().to_string())
                    .filter(|r| !r.is_empty());
                PermissionPromptDecision {
                    approved: false,
                    state: PermissionDecisionState::Reject,
                    denial_reason: reason,
                }
            }
            // Plain "Reject", ESC/`None`, or a timeout → reject (pi `permission-dialog.ts:155-158`).
            _ => PermissionPromptDecision {
                approved: false,
                state: PermissionDecisionState::Reject,
                denial_reason: None,
            },
        };
        AskOutcome::Decided(decision)
    }
}

/// pi `splitPromptLines` (`permission-dialog.ts:37-39`): split on `/\r\n|\r|\n/` — each of `\r\n`,
/// `\r`, `\n` is ONE boundary (so `\r\n` yields no spurious empty line between the two chars).
fn split_prompt_lines(value: &str) -> Vec<String> {
    value.replace("\r\n", "\n").replace('\r', "\n").split('\n').map(str::to_string).collect()
}

/// pi `formatPromptCompactionNotice` (`permission-dialog.ts:41-49`).
fn format_prompt_compaction_notice(omitted_lines: usize, omitted_characters: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    if omitted_lines > 0 {
        parts.push(format!("{omitted_lines} {}", if omitted_lines == 1 { "line" } else { "lines" }));
    }
    if omitted_characters > 0 {
        parts.push(format!(
            "{omitted_characters} {}",
            if omitted_characters == 1 { "character" } else { "characters" }
        ));
    }
    let summary = if parts.is_empty() { "content".to_string() } else { parts.join(" and ") };
    format!("[Permission prompt compacted: omitted {summary} to keep the permission dialog usable.]")
}

/// pi `compactPermissionPromptForSelect` (`permission-dialog.ts:51-84`): a UX guard capping the
/// dialog body to 32 lines / 2200 chars, appending a compaction notice. Pure; ported for fidelity.
fn compact_permission_prompt_for_select(value: &str) -> String {
    let lines = split_prompt_lines(value);
    if lines.len() <= PERMISSION_DIALOG_MAX_VISIBLE_LINES
        && value.chars().count() <= PERMISSION_DIALOG_MAX_VISIBLE_CHARACTERS
    {
        return value.to_string();
    }

    let max_prefix_lines = PERMISSION_DIALOG_MAX_VISIBLE_LINES.saturating_sub(1).max(1);
    let prefix_lines: Vec<String> = lines.iter().take(max_prefix_lines).cloned().collect();
    let omitted_lines = lines.len().saturating_sub(prefix_lines.len());
    let mut prefix = prefix_lines.join("\n");

    for _ in 0..3 {
        let omitted_characters = value.chars().count().saturating_sub(prefix.chars().count());
        let notice = format_prompt_compaction_notice(omitted_lines, omitted_characters);
        let separator_length = usize::from(!prefix.trim_end().is_empty());
        let max_prefix_characters = PERMISSION_DIALOG_MAX_VISIBLE_CHARACTERS
            .saturating_sub(notice.chars().count())
            .saturating_sub(separator_length);

        if prefix.chars().count() <= max_prefix_characters {
            let trimmed = prefix.trim_end();
            return if trimmed.is_empty() { notice } else { format!("{trimmed}\n{notice}") };
        }
        prefix = prefix.chars().take(max_prefix_characters).collect::<String>().trim_end().to_string();
    }

    let omitted_characters = value.chars().count().saturating_sub(prefix.chars().count());
    let notice = format_prompt_compaction_notice(omitted_lines, omitted_characters);
    let trimmed = prefix.trim_end();
    if trimmed.is_empty() {
        notice
    } else {
        format!("{trimmed}\n{notice}")
    }
}

// ------------------------------------------------------ the child→parent forwarding channel (P-4)

/// The CHILD-side ask-forwarding channel (pi `confirmPermission` subagent branch →
/// `waitForForwardedPermissionApproval`, `index.ts:1514-1518,1255-1355`). Installed as the gate's
/// `ask_channel` on a subagent child (`extension.rs` `new_forwarding_child`). When an ask-tier
/// decision fires with no local human, [`Self::confirm`] hands the prompt `message` to
/// [`crate::forwarding::wait_for_forwarded_approval`], which writes a nonce-bound REQUEST into the
/// PARENT session's spool (addressed by the `CYRUP_SUBAGENT_PARENT_SESSION` anchor) and BLOCKS on the
/// bound RESPONSE up to `timeout` (pi's `PERMISSION_FORWARDING_TIMEOUT_MS`, 10-min default;
/// [`crate::forwarding::resolve_child_wait_timeout`]). Any failure (no anchor, spool unavailable,
/// timeout) resolves to a DENY decision — so the gate always fail-CLOSES, never hangs, never allows.
///
/// The blocking wait is awaited by the gate UNDER a [`cyrup_ext::HostCtx::begin_human_wait`] P-3
/// guard (`extension.rs` `resolve_ask`), so the dispatcher's 5s invocation budget is SUSPENDED for
/// the (remote-human-latency, ≫5s) forward instead of firing and fail-OPENing the child's tool.
pub struct ForwardingAskChannel {
    /// The agent dir whose `sessions/permission-forwarding/…` subtree is the shared spool root (pi
    /// `PI_AGENT_DIR`; both parent and child resolve the same path).
    agent_dir: PathBuf,
    /// The child's blocking-wait bound (pi `PERMISSION_FORWARDING_TIMEOUT_MS`, `index.ts:1314`).
    timeout: Duration,
    /// The SHARED late-bound capability slot (the SAME `OnceLock` the owning extension's
    /// `set_host_services` fills). Read only for the requester session id metadata
    /// (`ctx.sessionManager.getSessionId()`, pi `index.ts:1259`); the forward never REQUIRES it (a
    /// child with no live backend forwards with a `"unknown"` requester id — binding keys on the
    /// PARENT id + nonce, not the requester).
    host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
}

impl ForwardingAskChannel {
    #[must_use]
    pub fn new(
        agent_dir: PathBuf,
        timeout: Duration,
        host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    ) -> Self {
        Self { agent_dir, timeout, host_services }
    }
}

#[async_trait]
impl AskChannel for ForwardingAskChannel {
    async fn confirm(&self, _title: &str, message: &str, _opts: PromptOpts) -> AskOutcome {
        // pi `resolvePermissionForwardingTargetSessionId` subagent branch (`permission-forwarding.ts:
        // 139-145`): the target is the direct parent, from the `CYRUP_SUBAGENT_PARENT_SESSION` anchor
        // (pi `PI_AGENT_ROUTER_PARENT_SESSION_ID`). Empty/absent ⇒ `wait_for_forwarded_approval`
        // resolves to deny (fail-closed), matching pi's null-target deny (`index.ts:1267-1272`).
        let target = std::env::var(cyrup_ext_subagents::PARENT_SESSION_ENV_VAR).unwrap_or_default();
        // pi `getSessionId(ctx)` (`index.ts:960-970`): the requester's own id, else `"unknown"`.
        let requester_session_id = self
            .host_services
            .get()
            .and_then(|s| s.session_id())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        // pi `getActiveAgentName(ctx) || … || "unknown"` (`index.ts:1284`). In cyrup's process-per-
        // subagent model the child IS its persona for its whole lifetime, so its active agent name is
        // the `CYRUP_SUBAGENT_AGENT_NAME` spawn anchor (the SAME var `extension.rs`
        // `resolve_agent_name_from_env` reads for the policy layers); absent/blank ⇒ `"unknown"`,
        // matching pi's `|| "unknown"` fallback. Display-only metadata in the parent prompt, never
        // part of the nonce binding.
        let requester_agent_name = std::env::var(cyrup_ext_subagents::AGENT_NAME_ENV_VAR)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        let decision = crate::forwarding::wait_for_forwarded_approval(
            &self.agent_dir,
            &target,
            &requester_session_id,
            &requester_agent_name,
            message,
            self.timeout,
        )
        .await;
        AskOutcome::Decided(decision)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;

    #[tokio::test]
    async fn noop_channel_never_allows() {
        let ch = NoOpAskChannel;
        let out = ch.confirm("t", "m", PromptOpts::default()).await;
        assert!(matches!(out, AskOutcome::NoLiveChannel));
    }

    #[tokio::test]
    async fn forwarding_channel_denies_when_no_parent_anchor() {
        // No `CYRUP_SUBAGENT_PARENT_SESSION` set in this test env ⇒ null target ⇒ fail-closed deny
        // (pi `index.ts:1267-1272`), never a hang, never an allow.
        let dir = tempfile::tempdir().unwrap();
        let ch = ForwardingAskChannel::new(
            dir.path().to_path_buf(),
            Duration::from_millis(200),
            Arc::new(OnceLock::new()),
        );
        // Guard against an ambient anchor leaking in from a parallel test process.
        let restore = std::env::var(cyrup_ext_subagents::PARENT_SESSION_ENV_VAR).ok();
        // SAFETY: test-local, immediately restored; this test does not run its forward against a real
        // sibling. Rust 2024 requires `unsafe` for env mutation.
        unsafe { std::env::remove_var(cyrup_ext_subagents::PARENT_SESSION_ENV_VAR) };
        let out = ch.confirm("Permission Required", "run bash 'rm -rf /'?", PromptOpts::default()).await;
        if let Some(v) = restore {
            unsafe { std::env::set_var(cyrup_ext_subagents::PARENT_SESSION_ENV_VAR, v) };
        }
        match out {
            AskOutcome::Decided(d) => {
                assert!(!d.approved, "no parent anchor must fail-CLOSE to deny");
                assert_eq!(d.state, PermissionDecisionState::Denied);
            }
            AskOutcome::NoLiveChannel => panic!("forwarding resolves a decision, never NoLiveChannel"),
        }
    }
}
