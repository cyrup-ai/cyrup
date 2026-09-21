//! The herdr status bridge — cyrup's fleet in herdr's sidebar.
//!
//! When cyrup runs inside a [herdr](https://github.com/herdr) pane, this module is what makes the
//! pane's row in herdr's Agents sidebar tell the truth: **who is working, who is blocked waiting
//! on you, who is done.** The transport lives in the workspace's one herdr client,
//! [`cyrup_herdr`]; everything here is the cyrup half — the state machine, the label rules, and
//! (in later files) the reporter task that puts them on the wire.
//!
//! # The two verbs, and why they are never collapsed
//!
//! herdr separates *semantic state* from *presentation*, and says so on the method:
//!
//! > `state` carries semantic agent state and affects waits, notifications, and rollups. Report
//! > display-only values separately through metadata.
//!
//! — `tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:717-718` (herdr @ `d59d060`,
//! v0.9.1).
//!
//! So [`state`] produces a [`state::StateReport`] destined for `pane.report_agent`
//! ([`cyrup_herdr::schema::panes::PaneReportAgentParams`]) and [`label`] produces a
//! [`label::MetadataText`] destined for `pane.report_metadata`
//! ([`cyrup_herdr::schema::panes::PaneReportMetadataParams`]). A refactor that folds the two into
//! one envelope breaks herdr's waits and rollups while still *looking* right on screen, which is
//! why both modules carry tests that fail on exactly that.
//!
//! # The identity this bridge reports under
//!
//! [`SOURCE`] and [`AGENT`] are not cosmetic — herdr routes on both, and the wrong pair is
//! silently dropped rather than rejected. See each constant for the rungs it clears.
//!
//! # Module map
//!
//! | file | what it owns |
//! |---|---|
//! | [`state`] | the pure lifecycle → [`cyrup_herdr::schema::PaneAgentState`] machine: refcounted `working`, and `blocked` from the human-wait gate |
//! | [`label`] | the metadata text, and **the privacy rule**: raw prompts never enter pane metadata |
//! | [`reporter`] | the tokio task: two coalescing lanes, wire-time `seq`, the TTL refresh, the release |
//! | [`consumer`] | `events.subscribe` + `session.snapshot` over [`cyrup_herdr::HerdrClient::bootstrap`]: pane-gone, and re-asserting a contradicted state |
//! | [`runtime`] | the four-state gate, the one process-wide handle, and the entry points `native_impl.rs` calls |
//! | [`view`] | `[CYRUP-EXCEEDS-UPSTREAM]` the OPT-IN `agent.view.set` projection: herdr's own sidebar, ordered by attention |
//!
//! The first two are pure: no clock, no I/O, no socket. Every input is a method call and every
//! output is a value, which is what lets the transition table and the privacy rule be pinned by
//! unit tests that need neither a herdr nor a socket.
//!
//! # Where the live half plugs in
//!
//! [`runtime::arm`] on `HostEvent::SessionStart`, [`runtime::shutdown`] on `SessionShutdown`, and
//! [`runtime::bridge`] on every edge in between
//! (`crates/cyrup-ext-subagents/src/extension/host/native_impl.rs`). Outside a herdr pane
//! [`runtime::arm`] answers `None` and every one of those edges is one `Option` test — see
//! [`runtime`]'s four-state table.
//!
//! # `kill` is already covered, and this module installs no handler
//!
//! herdr never reclaims agent state from a `cyrup:` source — no TTL
//! (`tmp/herdr/src/terminal/state.rs:18-25`), and a process-exit override that only fires for an
//! agent herdr can name (`:401-407`, and `"cyrup"` deliberately is not one, see [`AGENT`]). So a
//! cyrup that dies without releasing strands a `working` or `blocked` row until herdr restarts,
//! and the AUG called for a SIGTERM/SIGHUP guard here.
//!
//! **It would have been a regression.** `crates/cyrup/src/signals.rs:212-240,317-330` already
//! installs SIGTERM and SIGHUP for every host, and on the interactive host — the only host this
//! bridge ever arms on ([`runtime`]'s second row) — `first_delivery_exit_code` returns `None`
//! (`signals.rs:224-230`), so the handler fires the cancel token, the run loop breaks, and `main`
//! disposes the runtime, which fans `session_shutdown{quit}` out to every extension. The site that
//! carries that last step is `crates/cyrup/src/main.rs:729-731` — `runtime.dispose().await`
//! immediately after `run_interactive` returns, commented *"Runs even when the TUI loop errored
//! out"*. That reaches this crate's
//! `HostEvent::SessionShutdown` arm, which calls [`shutdown`] and releases the pane. A second
//! listener here would exit the process out from under that teardown — the exact race
//! `signals.rs:86-90` was rewritten to remove.
//!
//! What is genuinely uncovered is a SIGKILL and a *second* signal delivery, which hard-exits
//! (`signals.rs:300-305`). Neither reaches any handler in any language. The presentation half of
//! that is bounded by [`reporter::METADATA_TTL_MS`]; the state half is not, and is the honest
//! residual of this feature.

pub mod consumer;
pub mod label;
pub mod reporter;
pub mod runtime;
pub mod state;
pub mod view;

pub use label::{
    MAX_TASK_LABEL_CHARS, MAX_TITLE_TASK_CHARS, MAX_WORKFLOW_LABEL_DEPTH, MAX_WORKFLOW_LABEL_NODES,
    MetadataText, RunLabel, bounded_task_label, metadata_text, workflow_task_label,
};
pub use reporter::{METADATA_REFRESH, METADATA_TTL_MS, Metadata, Reporter, SeqCounter};
pub use runtime::{HerdrBridge, arm, arm_in, bridge, shutdown};
pub use state::{RunAttention, StateModel, StateReport};
pub use view::{AGENT_VIEW_ENV, AGENT_VIEW_LABEL};

/// The authority name every report from this bridge carries, for the whole life of the process.
///
/// **Not a `herdr:` prefix, and not by taste.** herdr reserves that prefix for its own registered
/// integrations and treats a `herdr:<known-agent>` pair as a *drop*:
/// `is_reserved_native_state_source` (`tmp/herdr/src/agent_resume.rs:100-113`) matches nine pairs
/// — `("herdr:claude","claude")`, `("herdr:codex","codex")`, `("herdr:copilot","copilot")`,
/// `("herdr:devin","devin")`, `("herdr:droid","droid")`, `("herdr:qodercli","qodercli")`,
/// `("herdr:qwen","qwen")`, `("herdr:cursor","cursor")`, `("herdr:grok","grok")` — and for those
/// the reported **state is silently discarded**, only the session reference being kept
/// (`tmp/herdr/src/app/actions.rs:1536-1565`). The neighbouring privilege table,
/// `full_lifecycle_hook_authority` (`tmp/herdr/src/detect/mod.rs:327-337`), is the one pi's host
/// process is registered in as `("herdr:pi","pi")`; cyrup is in neither, so it reports as itself.
///
/// The spelling is inside herdr's own source charset — `[A-Za-z0-9:._-]`, at most 80 characters
/// (`socket-api.mdx:794`) — and it is **stable**, which is load-bearing twice over: herdr's
/// sequence ledger is keyed by source (`accept_hook_report`, `tmp/herdr/src/terminal/state.rs:707-709`)
/// and a pane accepts sequenced reports from at most 32 distinct sources for its whole lifetime,
/// with clearing and expiry releasing nothing (`socket-api.mdx:798`). A per-report source would
/// burn that budget in under a minute.
pub const SOURCE: &str = "cyrup:subagents";

/// The agent label every report from this bridge carries.
///
/// herdr normalises an unknown label through verbatim after trimming
/// (`normalize_reported_agent_label`, `tmp/herdr/src/app/api_helpers.rs:191-200`), and `"cyrup"`
/// *is* unknown: `parse_agent_label` (`tmp/herdr/src/detect/mod.rs:188-228`) is a closed table of
/// known agent names and cyrup is not in it. That is deliberate rather than unlucky — an unknown
/// label makes `known_agent_label_conflicts_with_detected_agent`
/// (`tmp/herdr/src/terminal/state.rs:1629-1635`) unreachable, so herdr's screen detection can
/// never overrule what cyrup reports about itself.
///
/// The price of that is stated here because it shapes the release path a later file owns:
/// `hook_authority_is_effective` (`tmp/herdr/src/terminal/state.rs:1812-1817`) short-circuits to
/// `true` for a non-`full_lifecycle` source, agent state carries **no TTL at all**
/// (`HookAuthority`'s six fields are `source`, `agent_label`, `state`, `message`,
/// `reported_at` and `session_ref` — a stamp of WHEN, and no expiry to compare it against;
/// `tmp/herdr/src/terminal/state.rs:18-25`),
/// and the process-exit override only fires for a *named* agent
/// (`tmp/herdr/src/terminal/state.rs:401-407`). A cyrup that dies without releasing strands its
/// pane at `working` or `blocked` **permanently**. Releasing is correctness here, not hygiene.
pub const AGENT: &str = "cyrup";
