//! The client-side transport, loaded into every session: length-prefixed JSON framing
//! ([`framing`]), the wire data model ([`protocol`]), the per-session [`client::IntercomClient`],
//! and broker discovery/auto-spawn ([`spawn`]). A faithful port of `pi-intercom/broker/{framing,
//! client,spawn}.ts` + `types.ts`.

pub mod client;
pub mod framing;
pub mod protocol;
pub mod spawn;
