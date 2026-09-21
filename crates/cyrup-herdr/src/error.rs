//! Every way a herdr call can fail, kept distinguishable.
//!
//! Three properties this module exists to hold, each of which upstream gets wrong somewhere:
//!
//! 1. **herdr's error `code` is a bare `String` on the wire** — `ErrorBody` is
//!    `{code: String, message: String}` (`tmp/herdr/src/api/schema/response.rs:35-39`), and the app
//!    half alone produces 54 distinct codes through one `encode_error(id, code, message)` helper
//!    (`tmp/herdr/src/app/api/responses.rs:7`). It is NOT a closed enum, so [`ApiErrorCode`] carries
//!    [`ApiErrorCode::Other`] and a herdr upgrade that invents a code is a value, not a parse
//!    failure.
//! 2. **herdr's own `message` is reproduced verbatim, never a synthesised sentence.** herdr's own
//!    Rust client does exactly this (`tmp/herdr/src/api/client.rs:164`,
//!    `Self::ErrorResponse(response) => write!(f, "{}", response.error.message)`).
//! 3. **"unavailable" is not one thing.** pi's `HerdrErrorCode` (`src/inspectors/herdr/client.ts:3-9`
//!    @v0.68.0) folds a ~60-code space onto five names, and `normalizeCode` (`:35-41`) sends
//!    everything it does not recognise to `VALIDATION_ERROR`. That normalisation is kept in exactly
//!    one place in this workspace — `crates/cyrup-intercom/src/project_pane.rs:48-128`, where it
//!    backs a byte-identical upstream sentence — and deliberately not here.

use std::path::PathBuf;
use std::time::Duration;

/// Why no herdr server could be reached at all — as opposed to a server that answered with an
/// error, which is [`HerdrError::Api`].
///
/// Each arm is a state the caller can act on differently, and keeping them apart is the whole
/// point: "not in a pane" means *do nothing*, "no socket" means *fall back or report the path*,
/// and "binary missing" means *tell the user how to install*.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Unavailable {
    /// `HERDR_ENV` is not `"1"`, or `HERDR_PANE_ID` is absent/empty.
    ///
    /// Both halves are load-bearing. herdr launches a pane process with `HERDR_ENV=1` and
    /// `HERDR_SOCKET_PATH` set (`tmp/herdr/src/pane.rs:156`,
    /// `tmp/herdr/src/integration/env.rs:29`) but **removes** `HERDR_PANE_ID` for a
    /// `PaneLaunchIdentity::OmitPane` launch (`tmp/herdr/src/pane.rs:170-172`) — so a process can
    /// sit inside herdr, see a live socket, and still have no pane of its own to report against.
    #[error("not running inside a herdr pane (HERDR_ENV/HERDR_PANE_ID are not both set)")]
    NotInHerdrPane,
    /// A socket path was resolved but could not be connected: no server, a stale socket file, or
    /// `ECONNREFUSED`. The path is in the message because "herdr is not running" is not actionable
    /// and "nothing is listening on /home/u/.config/herdr/herdr.sock" is.
    #[error("no herdr server at {path}: {source}")]
    NoSocket {
        /// The path that was tried, after the [`crate::env`] resolution ladder.
        path: PathBuf,
        /// The underlying connect error.
        #[source]
        source: std::io::Error,
    },
    /// The CLI fallback ([`crate::cli::HerdrCli`]) tried to spawn herdr and the OS answered
    /// `NotFound`: there is no such binary, on `PATH` or at the path [`crate::cli::HERDR_BIN`]
    /// named.
    ///
    /// **This message is not an install hint, deliberately.** The hint pi prints is a byte-identical
    /// upstream sentence with exactly one home in this workspace — `HerdrLauncher::not_installed`
    /// (`crates/cyrup-intercom/src/project_pane.rs:362-368`), onto which `HerdrLauncher::rendered`
    /// maps this arm — and a second copy here would be a second thing to keep in sync. What this
    /// arm owes is the fact and the name that was tried, which is what a consumer with its own
    /// vocabulary needs.
    #[error("no herdr binary at {bin}")]
    BinaryMissing {
        /// The binary name or path that was spawned, after the [`crate::cli::HERDR_BIN`] ladder.
        bin: String,
    },
}

// One further arm belongs to this enum and is NOT here, because it has no constructor and this
// crate ships nothing it cannot reach:
//
//   * `ProtocolMismatch` — herdr's CLI refuses ANY protocol difference before every verb
//     (`tmp/herdr/src/cli.rs:786-801` → `cli/protocol_guard.rs:16-43`, error code
//     `protocol_mismatch`) and that code is CLI-only: `tmp/herdr/src/api/server.rs` never produces
//     it, so a socket client only ever meets it as text on a shelled-out herdr's stderr — where
//     `HerdrCli` already reports it, as [`ApiErrorCode::Other`] carrying that exact spelling, with
//     herdr's own explanatory message attached. Promoting it to a named arm would need a client
//     that compares protocol numbers itself, which nothing here does.
//
// `#[non_exhaustive]` above is what makes adding it additive rather than breaking.

/// One of herdr's error codes, with an open arm.
///
/// The listed variants are the ones a cyrup consumer can actually hit; the full produced set is
/// larger (54 from `encode_error` in `tmp/herdr/src/app/api/` alone) and grows with every herdr
/// release, which is exactly why [`Self::Other`] exists. Round-tripping through `Other` means a
/// herdr upgrade is never a hard failure here.
///
/// Not a `serde` type. `ErrorBody.code` is a bare `String` on the wire
/// (`tmp/herdr/src/api/schema/response.rs:35-39`) and is decoded as one; this enum is the
/// *interpretation* applied after the fact, via [`ApiErrorCode::from_wire`]. Deriving
/// `Deserialize` here would put a closed enum on the wire, which is the mistake the `Other` arm
/// exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApiErrorCode {
    /// The request did not deserialise into herdr's `Request` — **including an unknown method
    /// name**, because `Method` is a tagged enum and an unknown tag is a serde error
    /// (`tmp/herdr/src/api/server.rs:177-204`; `tmp/herdr/src/api/schema/tests.rs:420-424` asserts
    /// the `"unknown variant"` text). This is therefore the code that means *this herdr build does
    /// not have that method*, which is why [`HerdrError::is_unsupported_method`] keys on it.
    InvalidRequest,
    /// `tmp/herdr/src/app/api/` — the named pane id does not exist.
    PaneNotFound,
    /// No pane is focused, so a `--current`-shaped request has no target.
    NoActivePane,
    /// No workspace is focused.
    NoActiveWorkspace,
    /// The pane target was resolved against a stale generation.
    StalePaneTarget,
    /// A split was attempted and the UI refused it.
    PaneSplitFailed,
    /// Input could not be delivered to the pane.
    PaneSendFailed,
    /// The named tab id does not exist.
    TabNotFound,
    /// The named workspace id does not exist.
    WorkspaceNotFound,
    /// Params deserialised but failed herdr's own validation.
    InvalidParams,
    /// `pane.report_metadata` was given a source herdr does not accept.
    InvalidMetadataSource,
    /// A metadata token key or value violated herdr's schema pattern.
    InvalidMetadataToken,
    /// A metadata TTL was out of range.
    InvalidMetadataTtl,
    /// `pane.report_agent` named an agent herdr does not know.
    InvalidAgent,
    /// `pane.send_input` named a key herdr cannot encode
    /// (`tmp/herdr/src/app/api/panes.rs:1852`). **Nothing was written**: herdr encodes the whole
    /// input before it sends any of it, so this is a clean refusal, not a partial write.
    InvalidKey,
    /// `pane.split` was given an `env` entry with an empty key, an `=` in a key, or a NUL in
    /// either half (`tmp/herdr/src/app/api/env.rs:3-31`).
    InvalidEnv,
    /// `pane.close` would have closed a whole worktree group, and herdr wants the user to say so
    /// explicitly (`tmp/herdr/src/app/api/panes.rs:1880-1886`). The pane is **still open**.
    ConfirmationRequired,
    /// `pane.wait_for_output` was given a `regex` pattern that does not compile
    /// (`tmp/herdr/src/api/wait.rs:38-48`). Refused before the first read, so it fails at once
    /// rather than after the timeout.
    InvalidRegex,
    /// `agent.get` named a target no agent answers to
    /// (`tmp/herdr/src/app/agents.rs:298-301`).
    AgentNotFound,
    /// `agent.get` named a target more than one agent answers to; herdr's message lists every
    /// candidate (`tmp/herdr/src/app/agents.rs:302-322`).
    AgentTargetAmbiguous,
    /// `agent.view.set` or `agent.view.clear` was refused by `validate_agent_view`
    /// (`tmp/herdr/src/app/agent_view.rs:19-39`, raised at `src/app/api/agent_view.rs:13,19,51`):
    /// a `source` outside `[A-Za-z0-9:._-]{1,120}`, a `label` over 32 characters, a filter deeper
    /// than 8 levels or wider than 64 nodes, an empty `all`/`any`, an `in` outside 1..=32 values,
    /// a context value compared to the wrong ID field, or more than 8 sort fields.
    InvalidAgentView,
    /// `tmp/herdr/src/app/api/agents.rs:149` — the agent refused the write.
    AgentBlocked,
    /// `tmp/herdr/src/app/api/pane_graphics.rs:538` — the feature is compiled out or configured off.
    FeatureDisabled,
    /// `tmp/herdr/src/app/api/pane_graphics.rs:111` — another stream already owns the pane.
    StreamConflict,
    /// A read query exceeded herdr's size ceiling.
    QueryTooLarge,
    /// The UI could not service the request right now.
    UiBusy,
    /// A read raced a redraw.
    StaleContent,
    /// `tmp/herdr/src/app/api/agents.rs:96` and `tmp/herdr/src/api/server.rs:842` — herdr's own
    /// 5 s `APP_RESPONSE_TIMEOUT` elapsed. Distinct from [`HerdrError::Timeout`], which is *this*
    /// client's deadline elapsing with no line at all.
    Timeout,
    /// The server is shutting down (`tmp/herdr/src/api/server.rs:390`).
    ServerUnavailable,
    /// `tmp/herdr/src/api/server.rs:373` — `client_shell.surface.set` is refused on the raw socket
    /// by construction. It can never be ported; it is absent, not deferred.
    ConnectionLocalOnly,
    /// herdr failed internally (`tmp/herdr/src/api/server.rs:365`).
    InternalError,
    /// Any code this client does not name — a newer herdr, or one of the 30-odd codes no cyrup
    /// consumer reaches. Carries the wire spelling unchanged.
    Other(String),
}

impl ApiErrorCode {
    /// Interpret herdr's wire `code`.
    ///
    /// Every spelling this client does not name becomes [`Self::Other`] **carrying that exact
    /// spelling** — never a placeholder, and never folded onto a catch-all like pi's
    /// `normalizeCode` (`src/inspectors/herdr/client.ts:35-41` @v0.68.0), which sends every
    /// unrecognised code to `VALIDATION_ERROR` and so cannot tell `ui_busy` from `agent_blocked`.
    #[must_use]
    pub fn from_wire(code: &str) -> Self {
        match code {
            "invalid_request" => Self::InvalidRequest,
            "pane_not_found" => Self::PaneNotFound,
            "no_active_pane" => Self::NoActivePane,
            "no_active_workspace" => Self::NoActiveWorkspace,
            "stale_pane_target" => Self::StalePaneTarget,
            "pane_split_failed" => Self::PaneSplitFailed,
            "pane_send_failed" => Self::PaneSendFailed,
            "tab_not_found" => Self::TabNotFound,
            "workspace_not_found" => Self::WorkspaceNotFound,
            "invalid_params" => Self::InvalidParams,
            "invalid_metadata_source" => Self::InvalidMetadataSource,
            "invalid_metadata_token" => Self::InvalidMetadataToken,
            "invalid_metadata_ttl" => Self::InvalidMetadataTtl,
            "invalid_agent" => Self::InvalidAgent,
            "invalid_key" => Self::InvalidKey,
            "invalid_env" => Self::InvalidEnv,
            "confirmation_required" => Self::ConfirmationRequired,
            "invalid_regex" => Self::InvalidRegex,
            "agent_not_found" => Self::AgentNotFound,
            "agent_target_ambiguous" => Self::AgentTargetAmbiguous,
            "invalid_agent_view" => Self::InvalidAgentView,
            "agent_blocked" => Self::AgentBlocked,
            "feature_disabled" => Self::FeatureDisabled,
            "stream_conflict" => Self::StreamConflict,
            "query_too_large" => Self::QueryTooLarge,
            "ui_busy" => Self::UiBusy,
            "stale_content" => Self::StaleContent,
            "timeout" => Self::Timeout,
            "server_unavailable" => Self::ServerUnavailable,
            "connection_local_only" => Self::ConnectionLocalOnly,
            "internal_error" => Self::InternalError,
            other => Self::Other(other.to_owned()),
        }
    }

    /// herdr's wire spelling, round-tripping [`Self::Other`] unchanged.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::PaneNotFound => "pane_not_found",
            Self::NoActivePane => "no_active_pane",
            Self::NoActiveWorkspace => "no_active_workspace",
            Self::StalePaneTarget => "stale_pane_target",
            Self::PaneSplitFailed => "pane_split_failed",
            Self::PaneSendFailed => "pane_send_failed",
            Self::TabNotFound => "tab_not_found",
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::InvalidParams => "invalid_params",
            Self::InvalidMetadataSource => "invalid_metadata_source",
            Self::InvalidMetadataToken => "invalid_metadata_token",
            Self::InvalidMetadataTtl => "invalid_metadata_ttl",
            Self::InvalidAgent => "invalid_agent",
            Self::InvalidKey => "invalid_key",
            Self::InvalidEnv => "invalid_env",
            Self::ConfirmationRequired => "confirmation_required",
            Self::InvalidRegex => "invalid_regex",
            Self::AgentNotFound => "agent_not_found",
            Self::AgentTargetAmbiguous => "agent_target_ambiguous",
            Self::InvalidAgentView => "invalid_agent_view",
            Self::AgentBlocked => "agent_blocked",
            Self::FeatureDisabled => "feature_disabled",
            Self::StreamConflict => "stream_conflict",
            Self::QueryTooLarge => "query_too_large",
            Self::UiBusy => "ui_busy",
            Self::StaleContent => "stale_content",
            Self::Timeout => "timeout",
            Self::ServerUnavailable => "server_unavailable",
            Self::ConnectionLocalOnly => "connection_local_only",
            Self::InternalError => "internal_error",
            Self::Other(code) => code.as_str(),
        }
    }
}

impl std::fmt::Display for ApiErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An `{"id":…,"error":{"code":…,"message":…}}` envelope, decoded.
///
/// `message` is herdr's own text, byte for byte (`tmp/herdr/src/api/client.rs:164` does the same).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} ({code})")]
pub struct ApiError {
    /// herdr's `error.code`.
    pub code: ApiErrorCode,
    /// herdr's `error.message`, unmodified.
    pub message: String,
}

/// Every failure mode of a herdr socket call.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HerdrError {
    /// No server was reachable. See [`Unavailable`] for which of its three arms this is.
    #[error("{0}")]
    Unavailable(#[source] Unavailable),
    /// herdr answered, and the answer was an error envelope.
    #[error("herdr {method} failed: {source}")]
    Api {
        /// The wire method name, e.g. `"ping"`.
        method: &'static str,
        /// herdr's own code and message.
        #[source]
        source: ApiError,
    },
    /// The answer's `id` was not the `id` that was sent.
    ///
    /// The connection is dropped rather than the payload accepted: one request per connection
    /// (`tmp/herdr/src/api/server.rs:156-317`) means a mismatched id cannot be a pipelining
    /// artefact, so it is either a confused server or a crossed socket, and either way the payload
    /// describes something other than what was asked. pi drops it too
    /// (`src/runs/shared/herdr-connection.ts:86` @v0.68.0).
    #[error("herdr answered {method} with id {got:?}, expected {sent:?}")]
    IdMismatch {
        /// The wire method name.
        method: &'static str,
        /// The id this client generated.
        sent: String,
        /// The id herdr echoed.
        got: String,
    },
    /// herdr closed the connection without writing a line.
    #[error("herdr closed the connection before answering {method}")]
    Closed {
        /// The wire method name.
        method: &'static str,
    },
    /// This client's deadline elapsed with no complete line.
    ///
    /// There is no herdr-side deadline on a plain dispatch — `tmp/herdr/src/api/server.rs:911-913`
    /// is a `recv()` with `None` timeout — so without this the call would hang forever.
    #[error("herdr did not answer {method} within {timeout:?}")]
    Timeout {
        /// The wire method name.
        method: &'static str,
        /// The deadline that elapsed.
        timeout: Duration,
    },
    /// The line herdr wrote was not a JSON value this client could decode.
    #[error("herdr sent a line answering {method} that is not a valid response")]
    Malformed {
        /// The wire method name.
        method: &'static str,
        /// The serde failure.
        #[source]
        source: serde_json::Error,
    },
    /// A line exceeded a framing bound. See [`crate::transport`] for where each bound comes from.
    #[error("herdr {method} exceeded the {limit} byte line bound")]
    TooLarge {
        /// The wire method name.
        method: &'static str,
        /// The bound that was hit, in bytes.
        limit: usize,
    },
    /// herdr answered with a success whose `result.type` is not the one this method produces —
    /// including `type`s newer than this client, which decode to
    /// [`crate::schema::ResponseResult::Unrecognised`].
    ///
    /// **This is never silently upgraded to a success.** herdr's own client does the same
    /// (`tmp/herdr/src/api/client.rs:119`, `ApiClientError::UnexpectedResult`).
    #[error("herdr answered {method} with {got}, which is not a {want}")]
    UnexpectedResult {
        /// The wire method name.
        method: &'static str,
        /// The `result.type` this method produces.
        want: &'static str,
        /// What arrived instead.
        got: String,
    },
    /// Socket I/O that is not a connect failure (a connect failure is
    /// [`Unavailable::NoSocket`], which keeps the path).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl HerdrError {
    /// `true` when **herdr refused to deserialise this request** — which includes, but is not
    /// limited to, *this herdr build does not have that method*.
    ///
    /// herdr's stability rule is per-method, not per-version: *"Other missing methods disable only
    /// those actions and show a client-local notice; they do not disconnect the UI … JSON API
    /// clients should ignore unknown fields and handle unsupported methods as normal errors."*
    /// (`tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:951-959`). An unknown
    /// method name fails `Request` deserialisation and comes back as `invalid_request`
    /// (`tmp/herdr/src/api/server.rs:177-201`), so that one code is the whole signal herdr gives.
    ///
    /// **It is not a narrower signal than that, and this accessor does not pretend otherwise.**
    /// The `invalid_request` arm is the `Err` branch of `serde_json::from_str::<Request>(line)`
    /// (`server.rs:177-201`), and `Method` is adjacently tagged with `content = "params"`, so
    /// params are deserialised inside that same call. An unknown method tag, a missing required
    /// params field, a wrong JSON type and an enum value this herdr build does not know all
    /// produce the identical code. So `true` means *this build will not accept this request as
    /// sent*, and the honest reading is "turn this **request shape** off", not "this method does
    /// not exist". The message is where the two are told apart — herdr's own test pins the
    /// `"unknown variant"` wording for the method-tag case
    /// (`tmp/herdr/src/api/schema/tests.rs:420-424`) — so a caller that must distinguish them
    /// reads [`ApiError::message`] rather than trusting this boolean to have done it.
    ///
    /// What it does promise: the client is **not** dead. Whatever herdr rejected, it rejected one
    /// line and answered on it; the connection model is one request per connection anyway.
    #[must_use]
    pub fn is_unsupported_method(&self) -> bool {
        matches!(
            self,
            Self::Api {
                source: ApiError {
                    code: ApiErrorCode::InvalidRequest,
                    ..
                },
                ..
            }
        )
    }

    /// The [`Unavailable`] inside, when this failure is one.
    #[must_use]
    pub fn unavailable(&self) -> Option<&Unavailable> {
        match self {
            Self::Unavailable(reason) => Some(reason),
            _ => None,
        }
    }
}

impl From<Unavailable> for HerdrError {
    fn from(reason: Unavailable) -> Self {
        Self::Unavailable(reason)
    }
}

/// This crate's `Result`.
pub type Result<T> = std::result::Result<T, HerdrError>;
