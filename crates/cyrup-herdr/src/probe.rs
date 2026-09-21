//! `ping` — liveness, version, and capability, and the rule for degrading without lying.
//!
//! ## Gate on capability, never on a version string
//!
//! `ping` is the only machine-readable version signal on the socket:
//! `{version: String, protocol: u32, capabilities}` (`tmp/herdr/src/api/schema/response.rs:45-50`,
//! produced at `tmp/herdr/src/api/server.rs:356-363`). Two things about it are easy to get wrong.
//!
//! **`protocol` is not a JSON-method generation.** It is `PROTOCOL_VERSION`
//! (`tmp/herdr/src/protocol/wire.rs:20`, `22` today) and it guards herdr's **binary** attach/render
//! endpoint; `wire.rs:6-8` says so in its own module doc. herdr's *CLI* refuses any difference
//! before every verb (`tmp/herdr/src/cli.rs:786-801` → `cli/protocol_guard.rs:16-43`), but the CLI
//! is a herdr build talking to a herdr build of possibly different vintage. A cyrup client is
//! neither, and refusing a JSON call because a binary protocol moved would disable a working
//! feature for a reason that does not apply to it. So this client records `protocol` and does not
//! gate on it.
//!
//! **`version` is not a capability list either.** herdr's stated stability rule is per method:
//! *"Other missing methods disable only those actions and show a client-local notice; they do not
//! disconnect the UI … JSON API clients should ignore unknown fields and handle unsupported methods
//! as normal errors."* (`socket-api.mdx:951-959`). An unknown method name fails `Request`
//! deserialisation and comes back as `invalid_request` (`tmp/herdr/src/api/server.rs:177-204`).
//!
//! So the degradation rule is: **`ping` once, cache the [`Pong`], and treat `invalid_request` on a
//! specific method as "this build lacks that one method" — turn that one feature off and keep the
//! rest.** [`crate::HerdrError::is_unsupported_method`] is that test.
//!
//! pi's `supportsRawPanes` ≥ 0.7.5 string gate (`src/inspectors/herdr/client.ts:118-120` @v0.68.0)
//! is deliberately **not** reproduced here. It survives in exactly one place in this workspace,
//! `crates/cyrup-intercom/src/project_pane.rs:273-276`, because there it backs a byte-identical
//! upstream sentence.

use std::path::Path;
use std::time::Duration;

use crate::env::{EnvSource, HerdrPane};
use crate::error::{HerdrError, Result};
use crate::schema::{ResponseResult, ServerCapabilities};
use crate::transport::{self, DEFAULT_TIMEOUT};

/// The answer to `ping`, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pong {
    version: String,
    protocol: u32,
    capabilities: Option<ServerCapabilities>,
}

/// A flag in herdr's [`ServerCapabilities`] block.
///
/// These describe the **server process**, not its method inventory — see the module doc. They are
/// the whole of what `ping` can tell a client declaratively; everything else is learned by calling
/// the method and reading the error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Capability {
    /// `server.live_handoff` is available (`ServerCapabilities::live_handoff`).
    LiveHandoff,
    /// The server runs as a detached daemon (`ServerCapabilities::detached_server_daemon`).
    DetachedServerDaemon,
    /// The server honours explicit client-shell surface interest
    /// (`ServerCapabilities::surface_interest`).
    SurfaceInterest,
    /// The server supports endpoint health probes (`ServerCapabilities::health_check`).
    HealthCheck,
}

impl Pong {
    /// herdr's own version string, e.g. `"0.9.1"`.
    ///
    /// Recorded for logs and for the one upstream-sentence site that still needs it. **Not** a gate
    /// — see the module doc.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The **binary** client-shell protocol generation (`PROTOCOL_VERSION`). Recorded, not gated
    /// on.
    #[must_use]
    pub const fn protocol(&self) -> u32 {
        self.protocol
    }

    /// The raw capability block, absent on servers that predate it.
    #[must_use]
    pub const fn capabilities(&self) -> Option<&ServerCapabilities> {
        self.capabilities.as_ref()
    }

    /// Whether the server declares `capability`.
    ///
    /// A server that sent no capability block answers `false` for every flag — "not declared" is
    /// the same decision as "declared off", because in both cases the only safe action is not to
    /// use the feature. The one thing it must never be is `true`.
    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        let Some(capabilities) = &self.capabilities else {
            return false;
        };
        match capability {
            Capability::LiveHandoff => capabilities.live_handoff,
            Capability::DetachedServerDaemon => capabilities.detached_server_daemon,
            Capability::SurfaceInterest => capabilities.surface_interest,
            Capability::HealthCheck => capabilities.health_check,
        }
    }

    /// The server's stable client-owned endpoint generation, when it declares one.
    #[must_use]
    pub fn endpoint_protocol_generation(&self) -> Option<u32> {
        self.capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.endpoint_protocol_generation)
    }
}

/// `ping` the server at `socket`, with the default [`DEFAULT_TIMEOUT`] deadline.
///
/// `ping` is answered by the API server itself without touching the app
/// (`tmp/herdr/src/api/server.rs:355-368`), so it stays answerable while the UI is busy — which is
/// what makes it a liveness probe rather than another request.
///
/// # Errors
/// [`crate::Unavailable::NoSocket`] when nothing is listening; [`HerdrError::Timeout`] when the
/// server accepts but never answers; [`HerdrError::UnexpectedResult`] if the answer is a success
/// that is not a `pong`.
pub async fn ping(socket: &Path) -> Result<Pong> {
    ping_for(socket, DEFAULT_TIMEOUT).await
}

/// [`ping`] with an explicit deadline.
///
/// # Errors
/// As [`ping`].
pub async fn ping_for(socket: &Path, timeout: Duration) -> Result<Pong> {
    let request = crate::client::ping_request();
    let result = transport::request(socket, &request, timeout).await?;
    pong(result)
}

/// `ping` the server owning the pane this process runs in.
///
/// This is the **asked-for** path: a caller that has been told to do a herdr thing and must report
/// why it cannot. It is not the ambient path — nothing calls this unless a consumer decided to.
/// The inert default is [`HerdrPane::discover`], which returns `None` and starts nothing.
///
/// # Errors
/// [`crate::Unavailable::NotInHerdrPane`] when `HERDR_ENV`/`HERDR_PANE_ID` do not both say so;
/// otherwise as [`ping`].
pub async fn ping_current_pane(env: &impl EnvSource) -> Result<Pong> {
    let pane = HerdrPane::require(env)?;
    ping(pane.socket_path()).await
}

/// The typed accessor for a `ping` answer.
///
/// [`ResponseResult::Unrecognised`] — a `result.type` newer than this client — is an error here,
/// never a defaulted success. herdr's own client does the same
/// (`tmp/herdr/src/api/client.rs:119`, `ApiClientError::UnexpectedResult`).
fn pong(result: ResponseResult) -> Result<Pong> {
    match result {
        ResponseResult::Pong {
            version,
            protocol,
            capabilities,
        } => Ok(Pong {
            version,
            protocol,
            capabilities,
        }),
        other => Err(HerdrError::UnexpectedResult {
            method: "ping",
            want: "pong",
            got: other.type_name().to_owned(),
        }),
    }
}
