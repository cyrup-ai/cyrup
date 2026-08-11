//! The client-side transport, loaded into every session: length-prefixed JSON framing
//! ([`framing`]), the wire data model ([`protocol`]), transport-target selection ([`target`]), the
//! connection itself ([`stream`]), the per-session [`client::IntercomClient`], and broker
//! discovery/auto-spawn ([`spawn`]). A faithful port of `pi-intercom/broker/{framing,client,spawn,
//! paths}.ts` + `types.ts` (v0.7.0).
//!
//! All three of pi-intercom's transports are here: the Unix domain socket, the Windows named pipe,
//! and the opt-in Windows loopback TCP endpoint. [`target`] decides which one a given
//! platform + environment selects (`paths.ts:44-116`) and [`stream`] opens it
//! (`connectToBrokerTarget`, `client.ts:26-30`).

pub mod client;
pub mod framing;
pub mod protocol;
pub mod spawn;
pub mod stream;
pub mod target;
