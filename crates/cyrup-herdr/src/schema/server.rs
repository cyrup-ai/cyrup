//! Mirrors `tmp/herdr/src/api/schema/server.rs`.

use serde::{Deserialize, Serialize};

/// `PingParams` (`tmp/herdr/src/api/schema/server.rs:3-4`).
///
/// A distinct empty struct rather than [`super::EmptyParams`], exactly as herdr declares it — and
/// it still has to be **sent**: `{"id":"x","method":"ping"}` with no `params` is rejected. See
/// [`super::EmptyParams`] for why.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PingParams {}

/// `ServerCapabilities` (`tmp/herdr/src/api/schema/server.rs:16-30`), the optional block on a
/// `pong`.
///
/// **This is not a method inventory.** Nothing here says whether a given JSON method exists; that
/// is answered per method by an `invalid_request` error — see [`crate::probe`] and
/// [`crate::HerdrError::is_unsupported_method`]. These five fields describe the *server process*:
/// whether it can hand itself off live, whether it is a detached daemon, and what the binary
/// client-shell endpoint supports.
///
/// Every field but `live_handoff` carries `#[serde(default)]` in herdr, so an older server that
/// omits them decodes with `false`/`None` here too — the same forward-compatible behaviour, not a
/// reinterpretation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// `server.live_handoff` is available on this build.
    pub live_handoff: bool,
    /// This server runs as a detached daemon.
    #[serde(default)]
    pub detached_server_daemon: bool,
    /// Stable client-owned endpoint generation supported by this server.
    #[serde(default)]
    pub endpoint_protocol_generation: Option<u32>,
    /// Whether this server supports explicit client-shell surface interest.
    #[serde(default)]
    pub surface_interest: bool,
    /// Whether this server supports endpoint health probes.
    #[serde(default)]
    pub health_check: bool,
}
