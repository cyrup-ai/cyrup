//! cyrup-permission-system — a runtime allow / ask / deny policy layer over every tool call, ported
//! 1:1 from `pi-permission-system` (v0.7.1 TypeScript). Registered as a
//! [`cyrup_ext::NativeExtension`] (never a WASM guest — port doc §3), it consumes the
//! `before_tool_call` block seam (`cyrup-ext/src/hooks.rs:31-44`, R-08-010): a tool call resolving to
//! `deny` (or a fail-closed `ask`) is `Block`ed; `allow` proceeds.
//!
//! This build ships the full host-independent policy DECISION ENGINE (four layers, trusted-floor,
//! last-match-wins, wildcard + per-action/resource + bash-command + mcp-target matching), the two
//! approval stores, the extension config, prompt dedup, the deciding gate, the LIVE in-session human
//! dialog (P-1/P-3): an `ask` on a tool the interactive human drives surfaces the real
//! `HostServices::select`/`input` dialog ([`ask::LocalAskChannel`], pi `permission-dialog.ts`) under a
//! [`cyrup_ext::HostCtx::begin_human_wait`] dispatch-budget-forgiveness guard so a slow human answer
//! never fail-OPENs the gate — AND the child→parent ask-FORWARDING spool (P-4, [`forwarding`], pi
//! `permission-forwarding.ts` + `index.ts:1030-1504`): an `ask` firing inside a subagent CHILD writes
//! a nonce-bound request into the PARENT session's filesystem spool and blocks on the bound response,
//! while the PARENT's permission extension runs a spawned watcher that surfaces each forwarded prompt
//! to its human and writes the decision back. Fully wired at the three `crates/cyrup/src/main.rs`
//! session-build sites (opt-in per DI-5; the child loads the gate with [`ask::ForwardingAskChannel`],
//! the parent with [`ask::LocalAskChannel`] + the watcher).
//!
//! All four supplementary policy LAYERS the pi `tool_call` handler runs (pi `index.ts:2208-2499`) are
//! now wired and ENFORCING — none are stubbed: the **agent + projectAgent** layers keyed by the
//! resolved persona name ([`extension`]'s `resolve_agent_name`, from the `CYRUP_SUBAGENT_AGENT_NAME`
//! env var cyrup-ext-subagents threads at spawn — pi `resolveAgentName`, `index.ts:2033-2047`); the
//! **registry / unknown-tool** block against the full registry ([`cyrup_ext::HostServices::
//! all_tool_names`], pi `getAllTools` / `index.ts:2218-2228`); the **skill-read bypass** ([`skill`],
//! pi `index.ts:2230-2303`) sourced from the `before_agent_start` `<available_skills>` block; and the
//! **external-directory** guard ([`gate`], pi `index.ts:2310-2414`) sourced from `HostCtx.cwd`.
//!
//! The `before_agent_start` context-hygiene layer is ALSO wired (pi `index.ts:2134-2190`, port doc
//! §9): the handler shapes the active tool set via [`cyrup_ext::HostServices::set_active_tools`]
//! (driven by [`manager::PermissionManager::get_tool_permission`] + `has_allowed_skills`, and by
//! those two ONLY — pi `shouldExposeTool`, `index.ts:1791-1816` @v0.8.0, has a read/skills bypass and
//! no other; see `PERM-009`), RETURNS the sanitized system prompt as a `[mutate]`
//! ([`sanitize::tools`] strips the "Available tools:" section + denied guideline bullets;
//! [`sanitize::skills`] hides `ask`/`deny` skills from `<available_skills>` while KEEPING their
//! enforcement entries for the skill-read gate), and surfaces the `"yolo"` status pill
//! ([`status`], pi `status.ts`), cleared on shutdown. None ship as callerless primitives — every type
//! built here is reachable from the wired gate, the shaping seam, the forwarding transport, or its
//! integration tests.
//!
//! The extension's one HUMAN-visible registration is the `/permission-system` slash command (pi
//! `index.ts:1502-1512`), registered from `init` and serviced by
//! [`cyrup_ext::NativeExtension::execute_command`]. It is the cyrup expression of pi's settings
//! modal (`config-modal.ts:63-123`) over the same two rows, and it is what makes the v0.8.0 config
//! WRITE path reachable: [`PermissionSystemExtension::save_extension_config`] (pi
//! `saveExtensionConfig`, `index.ts:1402-1420`) and [`PermissionSystemExtension::set_yolo_mode`] /
//! [`PermissionSystemExtension::toggle_yolo_mode`] (pi `setYoloModeFromRuntimeApi`,
//! `index.ts:1422-1469`) both land in [`ExtensionConfig::save`], whose merge-into-the-existing-
//! document semantics — non-extension keys preserved, a corrupt file refused rather than clobbered,
//! a symlinked config written through — were previously unobservable because the crate had no
//! non-test caller for them at all (`tests/config_command.rs`).
//!
//! The AUDIT / DEBUG TRAIL is wired too ([`logging`], pi `logging.ts` + the `writeReviewEntry` call
//! sites throughout `index.ts`): setting `"debug": true` in the `config.json` this crate itself
//! materializes arms a JSONL trail at `<agent_dir>/cyrup-permission-system/logs/
//! cyrup-permission-system-debug.jsonl` (redirectable with `CYRUP_PERMISSION_SYSTEM_LOGS_DIR`),
//! one `{timestamp, extension, stream, event, ...details}` line per record. The `review` stream
//! carries every decision the gate reaches — `permission_request.blocked` (policy-denied and
//! confirmation-unavailable, at each of the main / skill-read / external-directory layers),
//! `.waiting`, `.approved`/`.denied`, `.auto_approved` (yolo), `.duplicate_reused` and
//! `.approval_persisted` — with prompts and denial reasons accompanied by their
//! `createSensitiveLogMetadata` digests; the `debug` stream carries lifecycle events
//! (`config.loaded`). This is the answer to "why was this tool blocked / who approved this", and it
//! is the reason `debug` is more than a shape-parity field.
//!
//! No-panic policy (arch-00 §8): the workspace lints deny unwrap/expect/panic/indexing; this
//! crate-level `#![deny(...)]` restates it. Tests `#[allow(...)]` at the module level.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

pub mod ask;
pub mod common;
pub mod dedup;
pub mod error;
pub mod evaluate;
pub mod ext_config;
pub mod extension;
pub mod forwarding;
pub mod gate;
pub mod jsonc;
pub mod logging;
pub mod manager;
pub mod ordered;
pub mod sanitize;
pub mod skill;
pub mod status;
pub mod stores;
pub mod types;
pub mod wildcard;
pub mod yolo_api;

pub use ask::{
    AskChannel, AskOutcome, ForwardingAskChannel, LocalAskChannel, NoOpAskChannel,
    PermissionDecisionState, PermissionPromptDecision, PromptOpts,
};
pub use error::PermissionError;
pub use ext_config::ExtensionConfig;
pub use extension::{
    is_installed, permission_extension_for_env, PermissionSystemExtension, CHILD_ENV_VAR,
    EXTENSION_ID, INSTALL_ENV_VAR, PERMISSION_SYSTEM_COMMAND,
};
pub use forwarding::{
    process_forwarded_requests, resolve_child_wait_timeout, spawn_forwarding_watcher,
    wait_for_forwarded_approval, ForwardedPermissionRequest, ForwardedPermissionResponse,
    ForwardingLocation, ProcessForwardedOptions, SharedExtensionConfig,
    PERMISSION_FORWARDING_TIMEOUT,
};
pub use manager::{ManagerPaths, PermissionManager};
pub use types::{
    CheckSource, PermissionCheckResult, PermissionState,
};
pub use yolo_api::{YoloModeControlOptions, YoloModeControlResult, DEFAULT_YOLO_CONTROL_SOURCE};
