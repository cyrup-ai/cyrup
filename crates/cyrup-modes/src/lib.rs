//! cyrup-modes — non-interactive runtime adapters (arch-11; conformance: func-11).
//!
//! Thin front-end adapters that drive the one [`cyrup_session_svc::AgentSession`] seam headlessly.
//! Every adapter writes to a caller-supplied sink and takes its input as a parameter, so the same
//! logic is exercised by a `String`/`Vec<u8>` buffer in tests and by real stdio in the binary.
//!
//! - [`run_print`] — PRINT mode: run one prompt to completion, emit the final assistant text
//!   (optionally tool activity) to the writer, exit. Human-oriented plain output (R-11-005).
//! - [`run_json`] — JSON mode: serialize each [`cyrup_session_svc::AgentSessionEvent`] as one JSON
//!   object per line (JSONL) — a stable machine-readable event stream (R-11-007).
//! - [`run_rpc`] — RPC mode: a bidirectional strict-LF JSONL protocol over a reader + writer; parse
//!   incoming [`SessionCommand`] requests, drive the session, and emit response/event lines
//!   ([`RpcOut`]). The headless server other tools embed (R-11-011…016).
//! - [`RpcClient`] — the OTHER end of that protocol (Pi `modes/rpc/rpc-client.ts`, SEAM-017): a
//!   typed, id-correlated client that either spawns the agent in RPC mode or attaches to an
//!   already-open transport, so an embedder never hand-rolls NDJSON framing.
//! - [`to_json_event`] / [`is_upstream_wire_event`] — the shared wire projection both [`run_json`]
//!   and [`run_rpc`] write every event through: the projection to the [`JsonAgentSessionEvent`]
//!   shape that actually goes on the stream, plus the filter that keeps cyrup-only events off it.
//!   Public so an embedder decoding the stream can reuse the same rules.
//! - [`write_raw_stdout`] / [`flush_raw_stdout`] — the retrying protocol-stream writer the sync
//!   sinks write through ([`run_print`], [`run_json`]; the rpc sink is a tokio `AsyncWrite` whose
//!   own readiness machinery plays that role), so a transient `EAGAIN`/`EWOULDBLOCK`/`ENOBUFS` on a
//!   non-blocking pipe never drops a protocol line (TOOL-037).
//!
//! The three modes are adapters over the same seam — no mode reaches behaviour the others
//! structurally cannot (the "one seam" invariant).
#![forbid(unsafe_code)]

mod error;
mod json;
mod json_event;
mod print;
pub mod raw_stdout;
mod rpc;
mod rpc_client;
#[cfg(test)]
mod tests;

pub use error::ModesError;
pub use json::run_json;
pub use json_event::{is_upstream_wire_event, to_json_event, JsonAgentSessionEvent};
pub use print::{run_print, PrintOptions};
pub use raw_stdout::{flush_raw_stdout, write_raw_stdout, RAW_STDOUT_RETRY_DELAY_MS};
pub use rpc::{run_rpc, QueueModeArg, RpcOut, RpcResponse, SessionCommand};
pub use rpc_client::{
    event_type, EventSubscription, ForkMessage, ModelInfo, RpcClient, RpcClientError,
    RpcClientOptions, DEFAULT_CLI_PATH, DEFAULT_IDLE_TIMEOUT_MS, REQUEST_TIMEOUT_MS,
};
