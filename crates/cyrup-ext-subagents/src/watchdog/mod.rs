//! The subagent **watchdog** — a port of `pi-subagents/src/watchdog/` @v0.43.0 (17 files).
//!
//! The watchdog is a second, independent reviewer that reads what the agent just did (the "turn
//! delta"), asks a model whether the work is going wrong, and — when it is — surfaces a `concern`
//! or a `blocker` into the transcript, optionally driving an automatic follow-up turn. It runs in
//! two roles: in the ORCHESTRATOR session (`register-main.ts`, this crate's
//! [`register_main`]) and inside a spawned SUBAGENT child (`register-child.ts`,
//! [`register_child`]). Both roles drive the SAME state machine — upstream's
//! `MainWatchdogRuntime` ([`runtime::MainWatchdogRuntime`]) — differing only in how the config is
//! resolved and where a warning is delivered.
//!
//! ## Module map (upstream file -> cyrup module)
//!
//! | upstream | cyrup |
//! |---|---|
//! | `types.ts` (198 lines) | [`types`] |
//! | `scope.ts` (62) | [`scope`] |
//! | `settings.ts` (568) | [`settings`] |
//! | `warning-format.ts` (73) | [`warning_format`] |
//! | `emission-guard.ts` (123) | [`emission_guard`] |
//! | `runtime.ts` (868) | [`runtime`] |
//! | `register-main.ts` (440) | [`register_main`] |
//! | `register-child.ts` (117) | [`register_child`] |
//!
//! ## Where it is wired
//!
//! Upstream calls its two registration entry points from exactly two places, and this port calls
//! its equivalents from the two cyrup analogs of those places:
//!
//! * `registerMainWatchdog(pi)` — `pi-subagents/src/extension/index.ts:375`, inside the
//!   orchestrator extension's registration body; disposed from `runtimeCleanup` (`:416`) and
//!   handed to the executor (`:438`). cyrup: [`crate::extension::SubagentsExtension`] owns the
//!   runtime, registers the command/renderer/subscriptions in `init` and drives it from
//!   `on_event`.
//! * `registerChildWatchdog(pi)` — `pi-subagents/src/runs/shared/subagent-prompt-runtime.ts:477`,
//!   inside `registerSubagentPromptRuntime`. cyrup:
//!   [`crate::prompt_runtime::SubagentPromptRuntime`]'s `init`/`on_event`.
//!
//! ## Off by default
//!
//! `DEFAULT_WATCHDOG_CONFIG.enabled` is `false` (`settings.ts:72`) and `main.enabled` inherits it,
//! so a session that never writes `subagents.watchdog` into a `settings.json` — and never runs
//! `/subagents-watchdog on` — installs the command and the subscriptions but performs no review,
//! makes no model call and emits no message. Every event handler's first act after refreshing the
//! config is `if (!this.isEnabled()) return`.

pub mod change_signature;
pub mod child_status;
pub mod emission_guard;
pub mod lsp_diagnostics;
pub mod model_selection;
pub mod permission_arbiter;
pub mod register_child;
pub mod register_main;
pub mod render;
pub mod review;
pub mod runtime;
pub mod scope;
pub mod settings;
pub mod tool_actions;
pub mod turn_delta;
pub mod types;
pub mod warning_format;

/// The current wall-clock instant as JS `new Date().toISOString()` renders it
/// (`YYYY-MM-DDTHH:MM:SS.mmmZ`) — the exact shape upstream stamps into
/// `WatchdogScopeEntry.createdAt` (`scope.ts:20`), `WatchdogWarningDetails.displayedAt`
/// (`runtime.ts:520,636`) and `WatchdogLspRuntimeSnapshot.updatedAt` (`runtime.ts:722,742,758`).
///
/// Delegates to the crate's existing formatter rather than re-deriving the calendar arithmetic; a
/// clock before the Unix epoch (only reachable if the host clock is set that way) formats as the
/// epoch itself rather than failing, since this value is display/ordering metadata and never a
/// decision input.
#[must_use]
pub(crate) fn now_iso8601() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    crate::background::run_status::format_iso8601_millis(ms)
}
