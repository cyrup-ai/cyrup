//! The standalone broker **process** — a 1:1 port of `pi-intercom/broker/broker.ts`.
//!
//! Dispatched as the hidden `cyrup __intercom-broker` subcommand (re-exec of `current_exe()`,
//! mirroring `cyrup-ext-subagents`' `__subagent-runner`). It binds the listen target resolved by
//! [`crate::transport::target::broker_listen_target`] — `<intercomDir>/broker.sock` on POSIX,
//! `\\.\pipe\cyrup-intercom-<agent dir>` on Windows — speaks length-prefixed JSON
//! ([`crate::transport::framing`]), routes
//! `send` frames child→broker→target by session identity, enforces the registration handshake +
//! caps + per-connection token bucket, tracks ask edges (mutual-ask refusal + prune), coalesces
//! presence, answers the health probe byte-identically, and auto-shuts-down 5 s after its last
//! client leaves (`broker.ts:286-296`).
//!
//! Transports: all three — the Unix domain socket (POSIX), the Windows named pipe, and the
//! **opt-in** loopback-TCP endpoint (`CYRUP_INTERCOM_TRANSPORT=tcp` / `CYRUP_INTERCOM_TCP=1`) — are
//! bound through [`listener::BrokerListener`], this port's stand-in for upstream's single
//! polymorphic `net.createServer().listen(LISTEN_TARGET)` (`broker.ts:123,149-152`). ICOM-015
//! landed the TCP arm's broker half: [`run`] publishes the kernel-chosen port plus this run's
//! `BROKER_STATE_ID` into `broker.port.json` (`broker.ts:131-141`) and unlinks it at shutdown
//! (`:1423-1427`), [`state::BrokerState::handle_frame`] enforces the `requiresEndpointAuth`
//! credential on `health`/`register` (`:284-305`), and `trustedLocal` now follows the bound
//! endpoint rather than the platform (`:365`).
//!
//! ## Layout
//!
//! This file is a facade. `broker.ts` is one file upstream and so was this one, until it reached
//! 3,292 lines; the modules below are the seams it was carrying. `state` holds `BrokerState` and
//! the bookkeeping every handler builds on; `dispatch` is the frame switch; `session`/`send`/
//! `receipts`/`presence`/`extensions` are the handlers, one per protocol concern, each an
//! `impl BrokerState` block; `mailbox` is offline delivery (`v0.10.1`); `conn` is one connection's
//! reader/writer pair and `lifecycle` the process itself; `frame`, `limits` and `js` are the shared
//! plumbing, the ported constants, and the JS value semantics the protocol echoes verbatim.
//!
//! Items are `pub(super)`, which from a child of `broker` means "visible throughout `broker`", so a
//! handler reaches `BrokerState`'s fields exactly as it did when they shared one file — while the
//! crate's public surface stays [`run`] alone.

pub(crate) mod listener;
pub(crate) mod ratelimit;
pub(crate) mod routing;
pub(crate) mod runtime_claim;

mod conn;
mod dispatch;
mod extension_state;
mod extensions;
mod frame;
mod js;
mod lifecycle;
mod limits;
mod mailbox;
mod presence;
mod receipts;
mod send;
mod session;
mod state;

#[cfg(test)]
mod scoped;
#[cfg(test)]
mod test_support;

pub use lifecycle::run;
