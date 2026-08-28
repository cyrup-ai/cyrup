//! The `subagents` extension (arch-SA §3.2/§6.8): the [`SubagentsExtension`] facade and
//! everything it drives. The `NativeExtension` impl itself — init/on_event/execute_command — is
//! `host::native_impl`.
//!
//! This is the crate's final integration point: it wires every already-implemented subsystem —
//! [`crate::discovery`] (resolve agent personas), [`crate::exec`]/[`crate::spawn`] (foreground OS-
//! subprocess run), [`crate::background`] (detached second-hop async run), [`crate::tui`]
//! (progress/notice folding), [`crate::registration`] (config layering, doctor, cost, profiles) —
//! into the one [`cyrup_ext::native::NativeExtension`] the `cyrup` binary registers
//! (`crates/cyrup/src/main.rs`'s three `with_native_extension` call sites).
//!
//! # The mandated mechanism (restated once at the seam this module tree owns)
//!
//! Every subagent execution this module tree drives is a genuine OS subprocess: the `subagent`
//! tool's foreground shape dispatches to [`crate::exec::run_sync`], which spawns a REAL child via
//! [`crate::spawn::SpawnedChild::spawn`]; the background shape dispatches to
//! [`crate::background::spawn_detached::spawn_detached_runner`], a genuine SECOND, detached OS
//! process hop that itself re-execs `cyrup __subagent-runner --config <path>`
//! (`crates/cyrup/src/subagent_runner_cmd.rs`), which in turn spawns further children through the
//! identical spawn boundary. There is no in-process nested agent turn loop anywhere in this module
//! tree, no in-process event-relay standing in for a child's own execution, and no extension-host
//! session-access seam beyond the one, narrow, sanctioned [`crate::fork_context`] dependency on
//! `cyrup-session` (§6.6). This module tree adds no new such seam.
//!
//! Where each half of that mechanism lives after the split: the foreground spawn is
//! `executor::paths::drive_foreground_run_sync` (the sole `crate::exec::run_sync` call site), the
//! detached second hop is `executor::background`'s `spawn_detached_runner` call, the tool's
//! dispatch table is `tool::routing`, and the fork-context seam named above is
//! `executor::resolve`'s `fork_resolver`.
//!
//! # Fork-context without a live session-manager handle (an honest, scoped limitation)
//!
//! [`cyrup_ext::native::NativeExtension`] instances are constructed and `init`-ed BEFORE the owning
//! session's `SessionManager` exists (`crates/cyrup-session-svc/src/builder.rs`'s `build()`
//! constructs `manager` at step 2b, well after
//! `for ext in self.native_extensions { host .load_native(ext).await?; }` would already have run if
//! extensions were loaded that early — in fact native extensions are loaded even later, at step 4b,
//! but still driven by a caller-supplied `Arc<dyn NativeExtension>` that was itself constructed by
//! the BINARY before `SessionBuilder::build()` is ever called). Per arch-SA §12 item 6/10
//! (confirmed against current source, not assumed): no wiring exists today to inject an
//! `AgentSessionServices`/live `SessionManager` handle into
//! [`cyrup_ext::native::InitApi`]/[`cyrup_ext::native::HostCtx`] at construction or dispatch time,
//! and building that new cross-crate seam is explicitly out of this integration task's scope (the
//! task brief is unambiguous that this crate's ONLY sanctioned session access is the direct,
//! already-built [`crate::fork_context::ForkContextResolver`] dependency on `cyrup-session` — never
//! a new extension-host session-access seam).
//!
//! This module tree resolves that gap the same way [`crate::fork_context`] itself is documented to
//! work: a THROWAWAY `SessionManager` handle, opened fresh per dispatch call from
//! [`cyrup_ext::native::HostCtx::cwd`] via [`cyrup_session::SessionManager::continue_recent`] (the
//! identical primitive `cyrup-session-svc`'s own builder uses for `SessionTarget::Continue`),
//! scoped under this extension's own `sessions` subdirectory of the resolved agent dir. This is NOT
//! a live, shared-with-the-orchestrator manager — it never mutates any in-memory state the running
//! session itself holds (R-SA-139/DI-SA-6 is satisfied trivially: there is no live in-memory state
//! to mutate, only a fresh on-disk read). If no persisted session exists yet at `cwd`,
//! `continue_recent` synthesizes an in-memory session with no leaf, and
//! [`crate::fork_context::ForkContextResolver::resolve`] correctly fails hard
//! (`ForkRequiresLeaf`/`ForkRequiresPersistedParent`) rather than silently downgrading to `Fresh` —
//! preserving DI-SA-2 exactly.

#[cfg(test)]
pub(crate) mod testsupport;

mod executor;
mod host;
// SUBA-075: crate-visible so `fork_context`'s ported `findModelInfo` can resolve a fork's candidate
// models against `registry_models` — the same catalog binding every model-facing command here uses.
pub(crate) mod models;
mod tool;
mod wait_tool;

pub use executor::requests::{
    BackgroundSingleRequest, BackgroundStepsSpec, ForegroundRunRequest, GraphRunOutcome,
    SingleRunOverrides, StatusViewSelector,
};
pub use executor::SubagentExecutor;
pub use host::registration::{
    is_installed, registration_mode_from_env, resolve_registration_mode, subagent_extension_for,
    subagent_extension_for_env, subagent_extension_for_env_with_channels, INSTALL_ENV_VAR,
    RegistrationMode,
};
pub use host::SubagentsExtension;
pub use tool::SubagentTool;
pub use wait_tool::WaitTool;

// The crate-internal surface. `crate::exec` reads [`TOOL_NAME`] and [`resolve_registration_mode`]
// (both above); `crate::exec::acceptance` re-renders [`sj_acceptance_override`] and
// `crate::registration::guide` asserts against [`subagent_actions`] — and each of those two reads
// happens only from a `#[cfg(test)]` context in its consumer, so the re-export carries the same
// gate rather than standing as a permanently-unused import under `-D warnings`. `WAIT_TOOL_NAME`
// needs no re-export at all: `wait_tool` is its only reader, so it stays `pub(crate)` where it is
// defined, its visibility unchanged from before the split.
#[cfg(test)]
pub(crate) use tool::schema::sj_acceptance_override;
#[cfg(test)]
pub(crate) use tool::text::subagent_actions;

/// The literal, stable extension id every registration/log/doctor surface refers to.
pub(crate) const EXTENSION_ID: &str = "subagents";

/// The single LLM-visible tool name (R-SA-128). Also the name a persona lists in its own `tools:`
/// to be granted nested delegation — pi's `fanoutAuthorized = declaredBuiltinTools.includes(
/// "subagent")` (`runs/shared/pi-args.ts:194`), read by [`crate::exec::build_attempt_spawn_plan`].
pub(crate) const TOOL_NAME: &str = "subagent";
