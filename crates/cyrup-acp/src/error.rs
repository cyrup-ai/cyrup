//! How a cyrup failure is presented to the ACP client, and the crate's own error type.
//!
//! Replaces pi-acp v0.0.33 `src/acp/auth-required.ts`'s `maybeAuthRequiredError` — ADR-0028
//! finding F4, and step 1 of its migration plan. Every handler signature is written against
//! [`AcpFailure`] so that a `String` error never appears in the tree and the substring ladder never
//! has a place to live.
//!
//! It has exactly **one** dependency on another module in this crate, and it is deliberate:
//! `From<AcpFailure> for agent_client_protocol::Error` calls
//! [`crate::config_options::auth_methods_for_error`] so that `ACP-016`'s `data.authMethods` payload
//! is attached at *every* site rather than only at the ones that remembered to call
//! [`AcpFailure::into_error`]. That function is pure and takes no [`crate::connection::ClientView`],
//! which is what makes the edge safe; the alternative — the conversion this module used to have,
//! which attached an empty list — was observed on a live `session/prompt` refusal, where the client
//! is told to authenticate and handed no method to authenticate with.

use agent_client_protocol::schema::v1::AuthMethod;
use cyrup_session_svc::SessionServiceError;

/// The `AUTH_REQUIRED` message, byte-for-byte from pi-acp v0.0.33 `auth-required.ts` (ACP-016).
///
/// The trailing period is upstream's and is load-bearing for the byte-exactness assertion. It is a
/// `const` rather than an inline literal because upstream builds the same string independently at
/// three call sites and a fourth would be one typo away.
pub const AUTH_REQUIRED_MESSAGE: &str = "Configure an API key or log in with an OAuth provider.";

/// `-32000`. Named so a reader does not have to know that
/// `agent_client_protocol::ErrorCode::AuthRequired` is that number, and asserted against the SDK's
/// own mapping in this module's tests.
pub const AUTH_REQUIRED_CODE: i32 = -32000;

/// `-32602`, JSON-RPC's `Invalid params`.
pub const INVALID_PARAMS_CODE: i32 = -32602;

/// `-32603`, JSON-RPC's `Internal error`.
pub const INTERNAL_ERROR_CODE: i32 = -32603;

/// How a cyrup failure is presented to the ACP client. ADR-0028 F4.
///
/// Port of the *decision* pi-acp v0.0.33 `auth-required.ts` makes, not of its mechanism: upstream
/// lowercases `String(err.message)` and returns auth-required if it **contains** any of eleven
/// substrings, two of which (`401`, `403`) are bare digit runs and the rest of which are common
/// English words. Here the decision is an exhaustive `match` on a typed error and never looks at
/// message text at all.
///
/// # [CYRUP-DELTA] — the sniffer is not ported, and that changes observable behaviour
///
/// **What differs.** Upstream classifies a bash tool's `permission denied` (an ordinary `EACCES` on
/// a script or a protected directory) as an authentication failure, and any tool output or provider
/// message containing the digits `401`/`403` with it — including
/// `maximum context length is 200000 tokens, however you requested 214031 tokens`, where the `403`
/// is inside `214031`. On `newSession`'s path that pairing also runs `cleanupFailedNewSession`,
/// which unlinks the session file. cyrup classifies none of those as auth failures.
///
/// **What it costs.** A genuine provider auth failure that arrives wrapped in one of
/// [`SessionServiceError`]'s `#[from]` transparent variants (`Core`, `Agent`, `Session`,
/// `Extension`, …) lands in the catch-all as [`AcpFailure::Internal`] and the client shows a
/// generic error instead of the Authenticate banner. That is the SAFE direction — under-reporting
/// auth rather than over-reporting it — and it is exactly why the catch-all must be `Internal` and
/// may never be widened to `AuthRequired`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcpFailure {
    /// The client should offer `authenticate` / show the auth banner. `detail` is the underlying
    /// message and rides in `data`, never in `message` — `message` is [`AUTH_REQUIRED_MESSAGE`].
    AuthRequired { detail: String },
    /// `-32602`. The client sent something this agent cannot act on.
    InvalidParams { message: String },
    /// `-32603`. Everything else.
    Internal { message: String },
}

impl AcpFailure {
    /// Classify a typed session-service failure. ADR-0028 F4's `classify`.
    ///
    /// The three auth-bearing variants are exactly the ones the gap analysis found reachable, and
    /// all three are **pre-flight**: a provider 401/403 that happens mid-turn never becomes an
    /// `Err` at all (`ProviderError::into_error_message` flattens it into an `AssistantMessage`
    /// with `StopReason::Error`), so it is classified at the turn-settle boundary instead — that is
    /// `ACP-022` and it is a different function with a different input, deliberately not folded in
    /// here.
    ///
    /// **`ACP-Q5`, decided: no.** Upstream's `not configured` substring also fires on MCP-server
    /// and extension configuration errors, which have nothing to do with provider credentials. The
    /// typed classifier declines to reproduce that — `SessionServiceError::Extension` and
    /// `::Config` fall to `Internal`. The cost is that a user whose MCP server is misconfigured no
    /// longer sees an Authenticate banner, which is the point.
    #[must_use]
    pub fn classify(err: &SessionServiceError) -> Self {
        use SessionServiceError as E;
        match err {
            // The three pre-flight credential states, typed. `AuthPreflightRefused` already carries
            // pi's own `formatNoApiKeyFoundMessage` text verbatim and `NoModelSelected`'s `Display`
            // is pi's `/login` -> `/model` guidance, so `detail` is upstream's own wording without
            // this module composing any.
            E::NoConfiguredAuth(detail) | E::AuthPreflightRefused(detail) => {
                AcpFailure::AuthRequired {
                    detail: detail.clone(),
                }
            }
            E::NoModelSelected => AcpFailure::AuthRequired {
                detail: err.to_string(),
            },

            // Client-supplied values this agent cannot act on. These are `-32602` rather than
            // `-32603` because the remedy is for the CLIENT to send something else.
            E::ModelNotFound(pattern) => AcpFailure::InvalidParams {
                message: format!("Unknown modelId: {pattern}"),
            },
            E::InvalidForkEntry(_) | E::ImportFileNotFound(_) => AcpFailure::InvalidParams {
                message: err.to_string(),
            },

            // Named states that are genuinely internal to this agent. Listed explicitly rather than
            // left to the catch-all so that adding a variant upstream is a decision someone makes
            // AT THIS MATCH — which is the whole point of F4 — and so the reader can see that each
            // was considered and declined for `AuthRequired`.
            //
            // `MissingSessionCwd` is the one worth naming: it is `session/prompt`'s realistic
            // failure for a session whose recorded cwd was deleted (`ACP-221`), and it must reach
            // the client as an error the connection survives, never as an auth prompt.
            E::MissingSessionCwd(_)
            | E::NoModelForSummarization
            | E::StreamingNeedsBehavior
            | E::ExtensionCommandNotQueueable(_)
            | E::NoActiveRun
            | E::NothingToCompact
            | E::AlreadyCompacted
            | E::CompactionCancelled
            | E::NoRuntimeHost(_)
            | E::SessionNotSaved
            | E::Io(_)
            | E::Bash(_)
            | E::Core(_)
            | E::Agent(_)
            | E::Session(_)
            | E::Compaction(_)
            | E::Config(_)
            | E::Resources(_)
            | E::Extension(_)
            | E::Context(_) => AcpFailure::Internal {
                message: err.to_string(),
            },
        }
    }

    /// Classify the flattened `error_message` of a run's terminal `AssistantMessage` (`ACP-022`).
    ///
    /// Port of pi-acp v0.0.33 `auth-required.ts`'s `maybeAuthRequiredError` @v0.0.33 at its
    /// **third** call site — the one that rejects the in-flight ACP turn when the provider fails
    /// mid-stream. [`AcpFailure::classify`] covers the other two, which are pre-flight and typed;
    /// this one cannot be, and the reason is stated in `classify`'s doc: request and stream
    /// failures are *never thrown*. `ProviderError::into_error_message`
    /// (`crates/cyrup-provider/src/error.rs`) flattens them into an `AssistantMessage` with
    /// `StopReason::Error` and this string, so `AgentSession::prompt` SUCCEEDS on a provider 401
    /// and the only place the failure is observable is the terminal message.
    ///
    /// # [CYRUP-DELTA] — the match is ANCHORED, and that is the whole point
    ///
    /// **What differs.** Upstream lowercases the message and asks whether it *contains* any of
    /// eleven substrings, two of which are the bare digit runs `401` and `403`. That reads
    /// `maximum context length is 200000 tokens, however you requested 214031 tokens` as a 403 and
    /// tells the user to log in when what they must do is shorten their prompt. Here the only
    /// shapes accepted are ones a cyrup type *produces*: `ProviderError::Http`'s
    /// `#[error("http {status}: {message}")]` at `401`/`403`, and `AuthError`'s three
    /// `#[error(..)]` prefixes, each matched from byte 0.
    ///
    /// **What it costs.** A provider whose SDK renders its own status line in a shape neither the
    /// anchored prefixes nor the delimited token below can read is classified
    /// [`AcpFailure::Internal`] instead of auth-required, and the client shows the provider's own
    /// sentence in an error response rather than the Authenticate banner. That is the safe
    /// direction — under-reporting auth, never over-reporting it — and it is the same trade
    /// `classify`'s catch-all makes.
    ///
    /// # The delimited status token, and why it is not upstream's `contains`
    ///
    /// One shape does not go through `ProviderError::Http` and is not hypothetical: AWS Bedrock's
    /// SDK renders its own status line, and a run with invalid credentials produces
    /// `//internal.amazon.com/…/: 403: {"message":"The security token … is invalid."}` — observed,
    /// reproducibly, as a whole turn that used to be answered `end_turn`. So a second rule accepts
    /// a status **token**: the six bytes `": 401: "` or `": 403: "`, colon-space-digits-colon-space.
    ///
    /// This is not the rule `ACP-022` forbids. What it forbids is upstream's bare `'403'`
    /// substring, whose failure is
    /// `maximum context length is 200000 tokens, however you requested 214031 tokens` — a digit
    /// run *inside a number*. A digit run cannot be surrounded by `: ` and `: `; a delimiter-bound
    /// token can only appear where something deliberately wrote a status code as a field. The
    /// `214031` row is in this module's tests and stays there.
    #[must_use]
    pub fn classify_terminal(error_message: &str) -> Self {
        // `ProviderError::Http { status, .. }` — the one shape that carries a status code in a
        // position this can read without guessing. Anchored at byte 0 and terminated by the `:`
        // that the `#[error]` format string puts there, so `214031` cannot participate.
        const HTTP_AUTH_PREFIXES: [&str; 2] = ["http 401:", "http 403:"];
        // `AuthError`'s three `#[error(..)]` strings, verbatim. These reach here as
        // `ProviderError::Auth(_)`, which is `#[error(transparent)]`, so the message IS one of
        // these three with no prefix of its own.
        const AUTH_ERROR_PREFIXES: [&str; 3] = [
            "oauth refresh failed for ",
            "credential store failure for ",
            "api key auth failed for ",
        ];
        // The delimited token — see the doc above.
        const AUTH_STATUS_TOKENS: [&str; 2] = [": 401: ", ": 403: "];

        let is_auth = HTTP_AUTH_PREFIXES
            .iter()
            .chain(AUTH_ERROR_PREFIXES.iter())
            .any(|prefix| error_message.starts_with(prefix))
            || AUTH_STATUS_TOKENS
                .iter()
                .any(|token| error_message.contains(token));

        if is_auth {
            AcpFailure::AuthRequired {
                detail: error_message.to_string(),
            }
        } else {
            AcpFailure::Internal {
                message: error_message.to_string(),
            }
        }
    }

    /// The JSON-RPC code this failure serialises as.
    #[must_use]
    pub fn code(&self) -> i32 {
        match self {
            AcpFailure::AuthRequired { .. } => AUTH_REQUIRED_CODE,
            AcpFailure::InvalidParams { .. } => INVALID_PARAMS_CODE,
            AcpFailure::Internal { .. } => INTERNAL_ERROR_CODE,
        }
    }

    /// Render as an ACP error, attaching the advertised auth methods to the auth-required arm.
    ///
    /// ACP-016 — `data = { "authMethods": [...] }` carries the **full** list, so a client that has
    /// not called `initialize` can still render the Authenticate button from the error alone. The
    /// error is built with `Error::new(..)` and never `Error::auth_required()`, because
    /// `From<ErrorCode> for Error` stamps strum's display string (`"Authentication required"`) into
    /// `message` and would silently lose [`AUTH_REQUIRED_MESSAGE`].
    ///
    /// `methods` is a parameter rather than being looked up here so that the caller decides whether
    /// the `_meta["terminal-auth"]` compat half is included — `ACP-012`/`ACP-054` gate that on the
    /// client's own probe, and upstream's `getAuthMethods()`-with-no-options call at the three
    /// error sites emits it even to a client that declined, which is the asymmetry `ACP-016` says
    /// to resolve deliberately.
    #[must_use]
    pub fn into_error(self, methods: &[AuthMethod]) -> agent_client_protocol::Error {
        match self {
            AcpFailure::AuthRequired { detail } => {
                agent_client_protocol::Error::new(AUTH_REQUIRED_CODE, AUTH_REQUIRED_MESSAGE)
                    .data(serde_json::json!({ "authMethods": methods, "detail": detail }))
            }
            AcpFailure::InvalidParams { message } => {
                agent_client_protocol::Error::new(INVALID_PARAMS_CODE, message)
            }
            AcpFailure::Internal { message } => {
                agent_client_protocol::Error::new(INTERNAL_ERROR_CODE, message)
            }
        }
    }
}

/// `From<AcpFailure> for Error`, with `ACP-016`'s auth methods attached.
///
/// This is the conversion every `?` and every `.into()` in the crate reaches, so it is the one
/// that has to be right. It used to pass `&[]`, on the reasoning that a handler holding no
/// [`ClientView`](crate::connection::ClientView) has nothing to advertise — and the observable
/// result was a live `session/prompt` refusal reading
/// `{"code":-32000,"message":"Configure an API key or log in with an OAuth provider.",`
/// `"data":{"authMethods":[],…}}`: the client is told to authenticate and handed nothing to
/// authenticate with, which is the exact failure `ACP-016` describes.
///
/// [`crate::config_options::auth_methods_for_error`] needs no view — see its own doc for the
/// `_meta` asymmetry that resolves — so there is no site left where the list is unreachable, and
/// [`AcpFailure::into_error`] remains for a caller that holds a real view and wants the shim.
impl From<AcpFailure> for agent_client_protocol::Error {
    fn from(failure: AcpFailure) -> Self {
        failure.into_error(&crate::config_options::auth_methods_for_error())
    }
}

impl From<&SessionServiceError> for AcpFailure {
    fn from(err: &SessionServiceError) -> Self {
        AcpFailure::classify(err)
    }
}

/// The crate's own error type: everything that goes wrong **inside** `cyrup-acp` before a
/// [`AcpFailure`] can be decided.
///
/// The split is ADR-0028's decision rule verbatim — "expected business outcome ⇒ a named enum
/// variant; technical failure that aborts ⇒ `Result<T, E>`". [`AcpFailure`] is the business
/// outcome the ACP protocol has a first-class representation for; this is the abort.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    /// The transport ended, or a frame could not be written. Carries the SDK's own error.
    ///
    /// `ACP-004` — a `BrokenPipe`/`NotConnected` write failure is the client closing the pipe,
    /// which is the ACP host's NORMAL termination, and `run_acp_dispatch`
    /// (`crates/cyrup/src/run.rs`) maps it to `Ok(())` rather than surfacing it here. Anything
    /// that reaches this variant is a real transport fault.
    ///
    /// **Where the io text actually is.** Every transport io failure on this path is built by
    /// `agent_client_protocol::Error::into_internal_error`, which is
    /// `Error::internal_error().data(err.to_string())` (agent-client-protocol-schema-1.7.0
    /// `src/v1/error.rs:132-136`), reached from the outgoing sink and the incoming stream alike
    /// (agent-client-protocol-2.1.0 `src/jsonrpc/transport_actor.rs:129,:139,:149,:229`). So the
    /// `Broken pipe (os error 32)` sits in the wire error's `data` field and `message` is
    /// the literal `"Internal error"` — [`AcpFailure::into_error`]'s own doc records the same
    /// strum trap from the other side, and a hang-up predicate that reads only `message` is dead
    /// code. That is exactly the defect `ACP-004` was filed for; `is_client_hangup` matches on the
    /// code plus `data`, with `message` kept only as a belt.
    #[error("acp transport: {0}")]
    Transport(agent_client_protocol::Error),

    /// A session-service call failed. Kept as the typed error rather than a string so
    /// [`AcpFailure::classify`] can still run on it at the boundary.
    #[error(transparent)]
    Session(#[from] SessionServiceError),

    /// The host could not build a session at all — the in-process replacement for pi-acp's
    /// `PiRpcSpawnError` and its three ENOENT/EACCES/other messages, none of which have a
    /// counterpart (gap-analysis 15 §3: there is no child process, and `ACP-001` re-enters the
    /// current executable, so ENOENT is structurally impossible).
    #[error("acp host: {0}")]
    Host(String),

    /// A path that had to be under a known root was not, or a session id failed
    /// `cyrup_session::validate_session_id`. See [`crate::ids`].
    #[error("acp path: {0}")]
    Path(String),

    /// A capability this connection needs is not attached — today, only "there is no client on
    /// this connection", which the detached [`crate::sessions::SessionManager`] uses to refuse a
    /// dialog rather than park a guest on a peer that does not exist.
    ///
    /// # Why there is no `Unimplemented` variant
    ///
    /// The foundation phase carried an `AcpError::Unimplemented { unit }` so an unwritten body
    /// could answer a value instead of a panic macro. Every such body is now written, and the variant
    /// is **deliberately deleted** rather than kept for future use: a spare "not implemented yet"
    /// constructor is how the next unfinished body gets shipped answering an error frame instead of
    /// being finished. The crate contains no skeleton marker and no panic macro, and there is no
    /// longer a type that would let an unfinished body return one.
    #[error("acp: {0}")]
    Detached(String),
}

impl From<AcpError> for AcpFailure {
    fn from(err: AcpError) -> Self {
        match err {
            AcpError::Session(ref inner) => AcpFailure::classify(inner),
            AcpError::Path(message) => AcpFailure::InvalidParams { message },
            other => AcpFailure::Internal {
                message: other.to_string(),
            },
        }
    }
}

impl From<AcpError> for agent_client_protocol::Error {
    fn from(err: AcpError) -> Self {
        AcpFailure::from(err).into()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use agent_client_protocol::ErrorCode;

    /// **ACP-022's canary.** The mid-turn classifier is ANCHORED, and the row that proves it
    /// matters is upstream's own defect: `214031` contains `403`.
    #[test]
    fn the_terminal_classifier_is_anchored_and_never_reads_a_digit_run() {
        let auth = |message: &str| {
            assert_eq!(
                AcpFailure::classify_terminal(message),
                AcpFailure::AuthRequired {
                    detail: message.to_string()
                },
                "{message}"
            );
        };
        let internal = |message: &str| {
            assert_eq!(
                AcpFailure::classify_terminal(message),
                AcpFailure::Internal {
                    message: message.to_string()
                },
                "{message}"
            );
        };

        // `ProviderError::Http`'s `#[error("http {status}: {message}")]` at the two auth statuses.
        auth("http 401: invalid x-api-key");
        auth("http 403: The security token included in the request is invalid.");
        // `AuthError`'s three `#[error(..)]` strings, which arrive verbatim through
        // `ProviderError::Auth`'s `#[error(transparent)]`.
        auth("oauth refresh failed for anthropic");
        auth("credential store failure for openai");
        auth("api key auth failed for together");

        // The observed AWS Bedrock shape: its SDK renders the status itself, so neither the
        // `http ` prefix nor the typed `AuthError` display is present. The delimited token is.
        auth(
            "//internal.amazon.com/coral/com.amazon.coral.service/: 403:              {\"message\":\"The security token included in the request is invalid.\"}",
        );
        auth("service.example: 401: unauthorized");

        // The row upstream gets wrong. `maybeAuthRequiredError` lowercases and asks `includes`,
        // so this reads as a 403 and tells the user to log in when what they must do is shorten
        // their prompt. A digit run inside a number cannot be surrounded by `: ` and `: `, which
        // is the whole reason the second rule is a delimited token rather than a substring.
        internal(
            "http 400: maximum context length is 200000 tokens, however you requested 214031 tokens",
        );
        internal("requested 403000 tokens");
        internal("status:403: forbidden-looking but not the token shape");
        // Every other unanchored substring in upstream's eleven-pattern table.
        internal("http 500: upstream exploded");
        internal("bash: /usr/local/bin/thing: permission denied");
        internal("mcp server \"docs\" is not configured");
        internal("http 429: rate limited, please retry in 401 seconds");
        // A message that merely CONTAINS the anchored prefix later on is not one.
        internal("tool output was: http 401: invalid x-api-key");
        // Total: an empty message is a failure with nothing to say, not an auth prompt.
        internal("");
    }

    /// The three named codes are the SDK's, not this module's guesses.
    #[test]
    fn the_named_codes_match_the_sdk() {
        assert_eq!(i32::from(ErrorCode::AuthRequired), AUTH_REQUIRED_CODE);
        assert_eq!(i32::from(ErrorCode::InvalidParams), INVALID_PARAMS_CODE);
        assert_eq!(i32::from(ErrorCode::InternalError), INTERNAL_ERROR_CODE);
    }

    /// ACP-016 — the payload is pinned byte-for-byte, and the construction detail that makes it so
    /// is asserted directly: `Error::auth_required()` alone produces strum's
    /// `"Authentication required"` and would be WRONG here.
    #[test]
    fn the_auth_required_payload_is_byte_exact() {
        let err = AcpFailure::AuthRequired {
            detail: "no api key for anthropic".into(),
        }
        .into_error(&[]);
        assert_eq!(i32::from(err.code), -32000);
        assert_eq!(
            err.message,
            "Configure an API key or log in with an OAuth provider."
        );
        assert_eq!(err.message, AUTH_REQUIRED_MESSAGE);
        let data = err.data.expect("auth_required carries data");
        assert!(data.get("authMethods").is_some(), "{data}");
        // The trap this const exists to avoid.
        assert_ne!(
            agent_client_protocol::Error::auth_required().message,
            AUTH_REQUIRED_MESSAGE,
            "`Error::auth_required()` stamps strum's display string; build the error by hand"
        );
    }

    /// ADR-0028 F4's table test. The three auth-bearing variants classify; everything named
    /// classifies to `Internal` or `InvalidParams` and NOTHING classifies to `AuthRequired` by
    /// accident.
    #[test]
    fn typed_auth_states_classify_and_nothing_else_does() {
        let auth = [
            SessionServiceError::NoConfiguredAuth("anthropic/claude".into()),
            SessionServiceError::AuthPreflightRefused("No API key found for anthropic".into()),
            SessionServiceError::NoModelSelected,
        ];
        for err in &auth {
            assert!(
                matches!(AcpFailure::classify(err), AcpFailure::AuthRequired { .. }),
                "{err} must classify as auth-required"
            );
        }
        assert_eq!(
            AcpFailure::classify(&SessionServiceError::ModelNotFound("gpt-9".into())),
            AcpFailure::InvalidParams {
                message: "Unknown modelId: gpt-9".into()
            }
        );
        assert!(matches!(
            AcpFailure::classify(&SessionServiceError::MissingSessionCwd("/gone".into())),
            AcpFailure::Internal { .. }
        ));
    }

    /// **The regression this type exists to prevent** — ACP-015's test (b), and the reason the
    /// sniffer must never come back.
    ///
    /// Every string here matches at least one of upstream's eleven substrings. Under
    /// `maybeAuthRequiredError` each becomes `RequestError.authRequired` — which on `newSession`'s
    /// path also unlinks the session file — and the user is told to reconfigure credentials that
    /// are fine.
    #[test]
    fn tool_output_that_looks_like_an_auth_failure_is_not_one() {
        let false_positives = [
            // `'permission denied'` — a routine EACCES from the bash tool.
            SessionServiceError::Io("bash: /usr/local/bin/deploy: permission denied".into()),
            // `'403'`, matched INSIDE `214031`. This exact sentence is the upstream defect.
            SessionServiceError::Io(
                "maximum context length is 200000 tokens, however you requested 214031 tokens"
                    .into(),
            ),
            // `'401'` inside an unrelated number, and `'forbidden'` as an ordinary English word.
            SessionServiceError::Io("wrote 401123 bytes".into()),
            SessionServiceError::Io("the forbidden directory was skipped".into()),
            // `'not configured'` — ACP-Q5's MCP/extension case, decided as NOT auth.
            SessionServiceError::Io("mcp server `docs` is not configured".into()),
            // `'authentication'` as prose in a tool's own output.
            SessionServiceError::Io("grep: authentication.rs: matched 3 lines".into()),
        ];
        for err in &false_positives {
            let classified = AcpFailure::classify(err);
            assert!(
                !matches!(classified, AcpFailure::AuthRequired { .. }),
                "{err} must NOT classify as auth-required (got {classified:?}) — this is the \
                 `maybeAuthRequiredError` substring ladder, and porting it is the defect"
            );
        }
    }

    /// Every `AcpError` reaches the client as a JSON-RPC frame rather than a panic, and the two
    /// non-session variants classify as `Internal` — never as the auth banner, which a client
    /// renders as "sign in" for what is an adapter fault.
    #[test]
    fn an_adapter_fault_is_an_internal_error_frame_not_a_panic() {
        for err in [
            AcpError::Host("no session host is installed on this connection".into()),
            AcpError::Detached("this ACP connection has no client attached".into()),
        ] {
            let text = err.to_string();
            let wire: agent_client_protocol::Error = err.into();
            assert_eq!(i32::from(wire.code), INTERNAL_ERROR_CODE, "{text}");
            assert!(!wire.message.is_empty(), "{text}");
        }
        // `Path` is the exception and it is deliberate: a containment refusal or a malformed
        // session id is the CLIENT's input, so it is `-32602`, not an adapter fault.
        let wire: agent_client_protocol::Error =
            AcpError::Path("outside the sessions root".into()).into();
        assert_eq!(i32::from(wire.code), INVALID_PARAMS_CODE);
    }
}
