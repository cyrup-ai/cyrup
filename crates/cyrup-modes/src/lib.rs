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
//!
//! All three are adapters over the same seam — no mode reaches behaviour the others structurally
//! cannot (the "one seam" invariant).
#![forbid(unsafe_code)]

mod error;
mod json;
mod json_event;
mod print;
mod rpc;

pub use error::ModesError;
pub use json::run_json;
pub use json_event::{to_json_event, JsonAgentSessionEvent};
pub use print::{run_print, PrintOptions};
pub use rpc::{run_rpc, QueueModeArg, RpcOut, RpcResponse, SessionCommand};
