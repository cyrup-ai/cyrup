//! cyrup-sdk — the public, embeddable API (arch-11; conformance: func-11 §7, R-11-019/023).
//!
//! A clean, ergonomic, **stable** surface for running cyrup in-process. It is a thin, documented
//! layer over [`cyrup_session_svc::AgentSession`] — the single integration seam every built-in
//! front-end consumes (R-11-023) — and adds **no behaviour**: it does not reimplement the agent
//! loop, persistence, tools, or extensions. It only wraps the facade with embedder-friendly
//! ergonomics and re-exports the load-bearing types so embedders need not depend on internal crates.
//!
//! # Quick start
//! ```no_run
//! use std::sync::Arc;
//! use cyrup_sdk::{Cyrup, SessionConfig};
//!
//! # async fn demo(provider: Arc<dyn cyrup_provider::Provider>) -> cyrup_sdk::SdkResult<()> {
//! // 1. Describe the run (cwd + agent dir + defaults).
//! let config = SessionConfig::new(".", "/home/me/.cyrup/agent");
//!
//! // 2. Build a session over a resolved provider.
//! let session = Cyrup::builder().build_session(provider, config).await?;
//!
//! // 3. Run a prompt to completion and read the final assistant text.
//! let answer = session.run("explain this codebase").await?;
//! println!("{answer}");
//! # Ok(()) }
//! ```
//!
//! # Streaming
//! ```no_run
//! # use futures::StreamExt;
//! # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
//! let mut events = session.prompt("write a haiku").await?;
//! while let Some(event) = events.next().await {
//!     println!("{}", event.kind());
//! }
//! # Ok(()) }
//! ```
//!
//! # Multi-session runtime + run modes
//! Beyond the single-session [`Session`] handle, the SDK re-exports the **multi-session runtime**
//! surface — [`AgentSessionRuntime`] + its construction primitive [`SessionFactory`] (and the
//! `new_session`/`switch_session`/`fork`/`reload`/`import`/`dispose` option/result types) — so an
//! embedder can build and drive a runtime that swaps the active session in place (R-11-020/021,
//! mirrors Pi's `createAgentSessionRuntime`). It also re-exports the **run-mode helpers**
//! [`run_print`]/[`run_json`]/[`run_rpc`] (mirrors Pi's `runPrintMode`/`runRpcMode`) so an embedder
//! can drive print/json/rpc over the seam (rpc over the runtime host) themselves.
//!
//! # What it re-exports
//! The load-bearing types an embedder needs are re-exported here (and in [`prelude`]):
//! [`AgentSessionEvent`], [`UserInput`], [`SessionConfig`], [`SessionTarget`], [`EventStream`],
//! [`SessionServiceError`], plus [`InputSource`], [`PromptAccepted`], [`StreamingBehavior`], and
//! [`AgentSessionServices`]; the runtime types above; and the run-mode helpers. The foundational
//! crates are also re-exported as modules ([`agent`], [`core`], [`modes`], [`provider`],
//! [`session`], [`session_svc`]).
#![forbid(unsafe_code)]

mod client;
mod error;
mod handle;

pub mod prelude;

// ---- the embedding surface ----
pub use client::{Cyrup, CyrupBuilder, zero_config_provider};
pub use error::{SdkError, SdkResult};
pub use handle::Session;

// ---- embedder-seam types: custom transport injection (Pi `AgentOptions.streamFn`, sdk.ts:301) +
// resource-override closures (Pi `DefaultResourceLoader` override closures, resource-loader.ts:125).
// `StreamFn` is the transport an embedder injects (bring-your-own proxy/HTTP client); `ProxyStreamFn`
// is the built-in proxy transport; `ApiKeyResolver` is dynamic per-request key resolution. The
// override closures transform the discovered [`SkillPointer`]/[`ContextFile`] sets. ----
/// Re-exported by name so an SDK user can match the transcript `handle.agent_messages()` returns
/// without reaching through the `agent` module alias.
pub use cyrup_agent::{AgentMessage, AppRole};
pub use cyrup_agent::{ApiKeyResolver, ProxyStreamFn, ProxyStreamOptions, StreamFn, stream_proxy};
pub use cyrup_session_svc::{ContextFile, ContextScope, SkillPointer};

// ---- load-bearing seam types, re-exported so embedders depend only on this crate ----
pub use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionServices, CompactionResult, InputSource, NoTools,
    PromptAccepted, ScopedModel, SessionBuilder, SessionConfig, SessionServiceError, SessionTarget,
    StreamingBehavior, UserInput,
};

// ---- multi-session runtime surface (R-11-020/021; Pi `agent-session-runtime.ts`,
// `createAgentSessionRuntime`) + the construction primitive an embedder builds it from
// ([`SessionFactory`], Pi `CreateAgentSessionRuntimeFactory`). An embedder constructs a factory over
// a resolved provider + base config, creates an [`AgentSessionRuntime`], and drives `new_session` /
// `switch_session` / `fork` / `reload` / `import_from_jsonl` / `dispose` — re-subscribing on each
// generation bump (R-11-021). This is the same layer the built-in interactive / RPC modes use.
pub use cyrup_session_svc::{
    AgentSessionRuntime, NewSessionOptions, RuntimeDiagnostic, RuntimeForkResult, SessionFactory,
    SwitchResult, SwitchSessionOptions,
};

// ---- run-mode helpers (arch-11 §2.3; Pi `runPrintMode`/`runRpcMode` SDK exports) so an embedder
// can drive print / json / rpc over the seam (or the runtime host, for rpc) themselves. ----
// `to_json_event`/`JsonAgentSessionEvent` are the json/rpc stdout wire projection (Pi
// `toJsonEvent`, `coding-agent/src/modes/json-event.ts`), exported for the same reason Pi exports
// it — `modes/index.ts:6` and `index.ts:337` — so an embedder writing its own event lane emits the
// same delta-only `message_update` the built-in modes do rather than re-deriving the projection.
pub use cyrup_modes::{
    JsonAgentSessionEvent, ModesError, PrintOptions, QueueModeArg, RpcOut, RpcResponse,
    SessionCommand, run_json, run_print, run_rpc, to_json_event,
};

/// The single streaming primitive (arch-00 §3.1): every event feed is an `EventStream<T>`.
pub use cyrup_core::EventStream;

// ---- foundational crates, re-exported as modules for full access when needed ----
pub use cyrup_agent as agent;
pub use cyrup_core as core;
pub use cyrup_modes as modes;
pub use cyrup_provider as provider;
pub use cyrup_session as session;
pub use cyrup_session_svc as session_svc;
