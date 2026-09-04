//! The authentication modes — manual OAuth (MCP-167/168), the single-shot
//! auto-auth latch (MCP-162) and `executeConnect` (MCP-161).
//!
//! See [`crate::proxy`] for the module overview.

use serde_json::Value;

use cyrup_core::{CancelToken, ToolResult};

use crate::abort::{is_abort_error, throw_if_aborted};
use crate::config::{OAuthGrantType, OAuthSetting};
use crate::errors::{McpError, McpResult};
use crate::proxy::discovery::execute_list;
use crate::proxy::env::{ConnectOutcome, ConnectionStatus, ProxyCtx};
use crate::proxy::error_vocab::McpErrorCode;
use crate::proxy::results::{
    details, details_err, disabled_result, get_auth_failed_message, get_auth_required_message,
    not_found_result, text_result,
};

// ==================================================================================================
// 9 · Manual OAuth modes (MCP-167, MCP-168)
// ==================================================================================================

/// `proxy-modes.ts:92` `getRedirectPort(authorizationUrl)`.
///
/// Parse the `redirect_uri` query parameter, then that URI's port, accepting only an integer. Any
/// parse failure yields nothing. `url::Url::port()` normalises a scheme's default port away exactly
/// as `new URL(...).port` does, so `http://localhost/cb` and `http://localhost:80/cb` both yield
/// nothing on both sides.
#[must_use]
pub fn get_redirect_port(authorization_url: &str) -> Option<u16> {
    let parsed = url::Url::parse(authorization_url).ok()?;
    let redirect_uri = parsed
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .map(|(_, value)| value.into_owned())?;
    if redirect_uri.is_empty() {
        return None;
    }
    url::Url::parse(&redirect_uri).ok()?.port()
}

/// `proxy-modes.ts:104` `formatManualAuthInstructions(serverName, authorizationUrl)`.
///
/// An array of literals joined by `\n`, with the empty strings preserved and the final `portNote`
/// dropped when empty via `.filter(Boolean)`. **`portNote` itself begins with `\n`**, so when a port
/// is parseable the rendered text has a **blank line before it** — that is the byte-exact shape, not
/// an accident.
#[must_use]
pub fn format_manual_auth_instructions(server_name: &str, authorization_url: &str) -> String {
    let port_note = get_redirect_port(authorization_url).map(|port| {
        format!("\nThe redirect URL will use local port {port}. On a remote server it is expected for that localhost page to fail locally; copy the address bar URL anyway.")
    });
    let mut lines: Vec<String> = vec![
        format!("MCP OAuth required for \"{server_name}\"."),
        String::new(),
        "Open this URL in your local browser:".to_string(),
        String::new(),
        authorization_url.to_string(),
        String::new(),
        "After approving, copy the full redirected localhost URL from your browser address bar and send it back with:".to_string(),
        format!("mcp({{ action: \"auth-complete\", server: \"{server_name}\", args: {{ redirectUrl: \"PASTE_REDIRECT_URL_HERE\" }} }})"),
        String::new(),
        "You can also pass just the `code` query parameter as `args: { code: \"PASTE_CODE_HERE\" }`. JSON-string args remain supported.".to_string(),
    ];
    if let Some(note) = port_note {
        // `.trimEnd()` upstream, then `.filter(Boolean)` — a note that trims to empty disappears.
        let trimmed = note.trim_end().to_string();
        if !trimmed.is_empty() {
            lines.push(trimmed);
        }
    }
    // `.filter(Boolean)` drops the empty separators too — upstream relies on that NOT happening for
    // the middle blanks because they are `""`… which IS falsy. Reproduced literally: JS filters
    // every empty string out, so the joined text has no blank lines from the literals; the blank
    // lines a reader sees come from `portNote`'s leading `\n` and from the multi-line strings.
    lines.retain(|line| !line.is_empty());
    lines.join("\n")
}

/// `proxy-modes.ts:344` `executeAuthStart(state, serverName, signal)`.
///
/// The manual-OAuth block is a copy-paste protocol for remote/headless sessions: the model prints an
/// authorization URL and instructs the human to paste back the redirect URL. `startAuth` returning
/// no `authorizationUrl` means the flow completed synchronously (client-credentials).
pub async fn execute_auth_start(
    ctx: &ProxyCtx,
    server_name: &str,
    cancel: &CancelToken,
) -> McpResult<ToolResult> {
    let owned = ctx.owned_signal(cancel);
    throw_if_aborted(
        &owned,
        ctx.owner().stop_reason().as_deref().map(String::as_str),
    )?;

    let Some(definition) = ctx.config().mcp_servers.get(server_name).cloned() else {
        return Ok(not_found_result("auth-start", server_name));
    };
    if definition.is_disabled() {
        return Ok(disabled_result("auth-start", server_name));
    }

    let started = async {
        let server_url = ctx.env.resolve_server_url(&definition)?;
        let Some(server_url) = server_url.filter(|url| !url.is_empty()) else {
            return Ok(None);
        };
        if !ctx.env.supports_oauth(&definition) {
            return Ok(None);
        }
        ctx.env
            .start_auth(server_name, &server_url, &definition, &owned)
            .await
            .map(Some)
    }
    .await;

    match started {
        Err(error) => {
            let message = error.to_string();
            let mut map = details_err("auth-start", McpErrorCode::AuthStartFailed);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("message".to_string(), Value::String(message.clone()));
            Ok(text_result(
                format!("Failed to start OAuth for \"{server_name}\": {message}"),
                map,
            ))
        }
        // A falsy URL or `!supportsOAuth(definition)` — one message, one code.
        Ok(None) => {
            let mut map = details_err("auth-start", McpErrorCode::OauthNotSupported);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            Ok(text_result(
                format!("Server \"{server_name}\" is not configured for OAuth over HTTP."),
                map,
            ))
        }
        Ok(Some(None)) => {
            let mut map = details("auth-start");
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("authenticated".to_string(), Value::Bool(true));
            Ok(text_result(
                format!("OAuth authentication successful for \"{server_name}\"."),
                map,
            ))
        }
        Ok(Some(Some(authorization_url))) => {
            let mut map = details("auth-start");
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert(
                "authorizationUrl".to_string(),
                Value::String(authorization_url.clone()),
            );
            Ok(text_result(
                format_manual_auth_instructions(server_name, &authorization_url),
                map,
            ))
        }
    }
}

/// `proxy-modes.ts:388` `executeAuthComplete(state, serverName, input, signal)`.
///
/// On success the connection is **closed** and the failure record cleared, so the next `connect`
/// uses the new token rather than the stale session that produced the `401`.
pub async fn execute_auth_complete(
    ctx: &ProxyCtx,
    server_name: &str,
    input: &str,
    cancel: &CancelToken,
) -> McpResult<ToolResult> {
    let owned = ctx.owned_signal(cancel);
    throw_if_aborted(
        &owned,
        ctx.owner().stop_reason().as_deref().map(String::as_str),
    )?;

    let Some(definition) = ctx.config().mcp_servers.get(server_name) else {
        return Ok(not_found_result("auth-complete", server_name));
    };
    if definition.is_disabled() {
        return Ok(disabled_result("auth-complete", server_name));
    }

    match ctx
        .env
        .complete_auth_from_input(server_name, input, &owned)
        .await
    {
        Err(error) => {
            let message = error.to_string();
            let mut map = details_err("auth-complete", McpErrorCode::AuthCompleteFailed);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("message".to_string(), Value::String(message.clone()));
            Ok(text_result(
                format!("Failed to complete OAuth for \"{server_name}\": {message}"),
                map,
            ))
        }
        Ok(status) if status != "authenticated" => {
            let mut map = details_err("auth-complete", McpErrorCode::NotAuthenticated);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("status".to_string(), Value::String(status));
            Ok(text_result(
                format!("OAuth authentication did not complete for \"{server_name}\"."),
                map,
            ))
        }
        Ok(_) => {
            ctx.env.close(server_name).await;
            ctx.env.clear_failure(server_name);
            ctx.env.update_status_bar();
            let mut map = details("auth-complete");
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("authenticated".to_string(), Value::Bool(true));
            Ok(text_result(
                format!(
                    "OAuth authentication successful for \"{server_name}\". Run mcp({{ connect: \"{server_name}\" }}) to connect with the new token."
                ),
                map,
            ))
        }
    }
}

// ==================================================================================================
// 10 · `attemptAutoAuth` and the single-shot latch (MCP-162)
// ==================================================================================================

/// `proxy-modes.ts:122` `attemptAutoAuth`'s three-valued return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoAuthResult {
    /// Not attempted — `autoAuth` is off, the definition is missing/disabled/non-OAuth, or there is
    /// no URL to authenticate against.
    Skipped,
    /// A token was obtained; the caller closes and reconnects.
    Success,
    /// The attempt was made (or refused for a *reason the model must see*) and failed.
    Failed(String),
}

/// `proxy-modes.ts:122` `attemptAutoAuth(state, serverName, signal)`.
///
/// 1. `settings.autoAuth !== true` ⇒ **skipped**. Opt-in, not opt-out.
/// 2. Missing / disabled / non-OAuth definition ⇒ skipped.
/// 3. `resolveServerUrl(definition)` **throwing** (a missing `${VAR}`, an invalid URL after
///    interpolation) ⇒ **failed**, not skipped. A falsy URL ⇒ skipped. Reproduce that split.
/// 4. `grantType = definition.oauth?.grantType ?? "authorization_code"`. **No interactive UI and a
///    grant type other than `client_credentials`** ⇒ failed, with the message routed through
///    [`get_auth_required_message`] so a configured `settings.authRequiredMessage` still wins.
/// 5. `authenticate(...)`. Upstream's four-way branch exists only to avoid passing `undefined`
///    keys; Rust builds one options struct, so the seam takes one call.
/// 6. Abort errors **rethrow**; anything else ⇒ failed with [`get_auth_failed_message`].
///
/// The browser opening this may trigger is `opener::open` called directly by the native crate —
/// there is no host verb for it and none is wanted, matching upstream's direct `open` dependency.
pub async fn attempt_auto_auth(
    ctx: &ProxyCtx,
    server_name: &str,
    cancel: &CancelToken,
) -> McpResult<AutoAuthResult> {
    if !ctx.settings().auto_auth() {
        return Ok(AutoAuthResult::Skipped);
    }
    let Some(definition) = ctx.config().mcp_servers.get(server_name).cloned() else {
        return Ok(AutoAuthResult::Skipped);
    };
    if definition.is_disabled() || !ctx.env.supports_oauth(&definition) {
        return Ok(AutoAuthResult::Skipped);
    }

    let server_url = match ctx.env.resolve_server_url(&definition) {
        Err(error) => {
            return Ok(AutoAuthResult::Failed(get_auth_failed_message(
                ctx.settings(),
                server_name,
                &error.to_string(),
            )));
        }
        Ok(url) => url,
    };
    let Some(server_url) = server_url.filter(|url| !url.is_empty()) else {
        return Ok(AutoAuthResult::Skipped);
    };

    let grant_type = match definition.oauth.as_ref() {
        Some(OAuthSetting::Config(config)) => config
            .grant_type
            .unwrap_or(OAuthGrantType::AuthorizationCode),
        _ => OAuthGrantType::AuthorizationCode,
    };
    if !ctx.has_ui() && grant_type != OAuthGrantType::ClientCredentials {
        return Ok(AutoAuthResult::Failed(get_auth_required_message(
            ctx.settings(),
            server_name,
        )));
    }

    match ctx
        .env
        .authenticate(server_name, &server_url, &definition, cancel)
        .await
    {
        Ok(()) => Ok(AutoAuthResult::Success),
        Err(error) => {
            if is_abort_error(&error, Some(cancel)) {
                return Err(error);
            }
            Ok(AutoAuthResult::Failed(get_auth_failed_message(
                ctx.settings(),
                server_name,
                &error.to_string(),
            )))
        }
    }
}

// ==================================================================================================
// 11 · `executeConnect` (MCP-161)
// ==================================================================================================

/// `proxy-modes.ts:730` `executeConnect(state, serverName, signal)`.
///
/// Reconnect if already connected, else connect; one auto-auth retry on `needs-auth`; then an
/// **eight-step metadata commit in this order** — store metadata, prompts *iff* discovery succeeded,
/// instructions set-or-**delete**, cache write, notify with reason `"proxy-connect"`, keep-alive
/// mark, clear failure, status bar — and finally **[`execute_list`]'s output**, so a successful
/// connect reports `details.mode === "list"`.
pub async fn execute_connect(
    ctx: &ProxyCtx,
    server_name: &str,
    cancel: &CancelToken,
) -> McpResult<ToolResult> {
    let owned = ctx.owned_signal(cancel);
    throw_if_aborted(
        &owned,
        ctx.owner().stop_reason().as_deref().map(String::as_str),
    )?;

    if !ctx.config().mcp_servers.contains_key(server_name) {
        let mut map = details_err("connect", McpErrorCode::NotFound);
        map.insert("server".to_string(), Value::String(server_name.to_string()));
        return Ok(text_result(
            format!("Server \"{server_name}\" not found. Use mcp({{}}) to see available servers."),
            map,
        ));
    }
    if ctx.is_disabled(server_name) {
        return Ok(disabled_result("connect", server_name));
    }

    let outcome: McpResult<ConnectOutcome> = async {
        ctx.set_status(&format!("connecting to {server_name}..."));
        let current = ctx.env.get_connection(server_name);
        let mut connection = if current == Some(ConnectionStatus::Connected) {
            ctx.env.reconnect(server_name, &owned).await?
        } else {
            ctx.env.connect(server_name, &owned).await?
        };
        if connection.needs_auth() {
            match attempt_auto_auth(ctx, server_name, &owned).await? {
                AutoAuthResult::Failed(message) => {
                    return Err(McpError::Other(format!("\u{0}auth_required\u{0}{message}")));
                }
                AutoAuthResult::Success => {
                    ctx.env.close(server_name).await;
                    throw_if_aborted(
                        &owned,
                        ctx.owner().stop_reason().as_deref().map(String::as_str),
                    )?;
                    connection = ctx.env.connect(server_name, &owned).await?;
                }
                AutoAuthResult::Skipped => {}
            }
            if connection.needs_auth() {
                let message = get_auth_required_message(ctx.settings(), server_name);
                return Err(McpError::Other(format!("\u{0}auth_required\u{0}{message}")));
            }
        }
        Ok(connection)
    }
    .await;

    let connection = match outcome {
        Ok(connection) => connection,
        Err(error) => {
            // The `auth_required` arms are not connect failures: they return before `recordFailure`
            // and carry their own code. The NUL-delimited marker keeps that control flow inside one
            // `?`-friendly future without inventing a second error type for a two-branch catch.
            if let Some(message) = strip_auth_required_marker(&error) {
                let mut map = details_err("connect", McpErrorCode::AuthRequired);
                map.insert("server".to_string(), Value::String(server_name.to_string()));
                map.insert("message".to_string(), Value::String(message.clone()));
                return Ok(text_result(message, map));
            }
            // Upstream's catch does NOT rethrow an abort: it reports it as `error: "aborted"` and
            // skips `recordFailure`, because misclassifying a user cancellation as a connection
            // failure would poison the next minute of that server's availability. The only throw on
            // this path is the `throwIfAborted` at the top.
            let message = error.to_string();
            let aborted = is_abort_error(&error, Some(&owned));
            if !aborted {
                ctx.env.record_failure(server_name, &message);
            }
            ctx.env.update_status_bar();
            let code = if aborted {
                McpErrorCode::Aborted
            } else {
                McpErrorCode::ConnectFailed
            };
            let mut map = details_err("connect", code);
            map.insert("server".to_string(), Value::String(server_name.to_string()));
            map.insert("message".to_string(), Value::String(message.clone()));
            return Ok(text_result(
                format!("Failed to connect to \"{server_name}\": {message}"),
                map,
            ));
        }
    };

    // The eight-step commit, in order.
    ctx.with_metadata_mut(|metadata| {
        metadata.insert(server_name.to_string(), connection.metadata.clone());
    });
    if !connection.prompt_discovery_failed {
        ctx.env.commit_prompt_metadata(server_name);
    }
    if let Ok(mut instructions) = ctx.state.server_instructions.lock() {
        match connection
            .instructions
            .as_ref()
            .filter(|text| !text.is_empty())
        {
            // `state.serverInstructions.set(...)` / `.delete(...)` — the delete arm is what keeps a
            // server that dropped its instructions from showing yesterday's.
            Some(text) => {
                instructions.insert(server_name.to_string(), text.clone());
            }
            None => {
                instructions.shift_remove(server_name);
            }
        }
    }
    ctx.env.update_metadata_cache(server_name);
    ctx.state
        .notify_tool_metadata_updated(server_name, "proxy-connect");
    ctx.env.mark_keep_alive_after_connect(server_name);
    ctx.env.clear_failure(server_name);
    ctx.env.update_status_bar();
    Ok(execute_list(ctx, server_name))
}

/// The `\0auth_required\0<message>` marker [`execute_connect`]'s inner future uses to distinguish an
/// auth refusal from a connect failure. A NUL is chosen because it cannot occur in a server message.
fn strip_auth_required_marker(error: &McpError) -> Option<String> {
    let McpError::Other(text) = error else {
        return None;
    };
    text.strip_prefix("\u{0}auth_required\u{0}")
        .map(str::to_string)
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
    use crate::config::{McpConfig, McpSettings, ServerEntry};
    use crate::proxy::call::execute_call;
    use crate::proxy::env::ProxyEnv;
    use crate::proxy::results::default_auth_required_message;
    use crate::proxy::testsupport::{FakeEnv, config_with, ctx_with, http, stdio, text_of};
    use serde_json::json;
    use std::sync::atomic::Ordering;

    // ---- MCP-167 · manual OAuth text -----------------------------------------------------------------

    #[test]
    fn manual_auth_instructions_are_byte_exact_with_and_without_a_port() {
        let with_port = format_manual_auth_instructions(
            "linear",
            "https://auth.example.com/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Fcallback",
        );
        assert_eq!(
            get_redirect_port(
                "https://auth.example.com/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Fcallback"
            ),
            Some(8976)
        );
        assert!(with_port.starts_with("MCP OAuth required for \"linear\".\n"));
        // `portNote` begins with `\n`, so the rendered text carries a BLANK LINE before it.
        assert!(
            with_port.contains("supported.\n\nThe redirect URL will use local port 8976."),
            "expected a blank line before the port note:\n{with_port}"
        );
        assert!(with_port.contains(
            "mcp({ action: \"auth-complete\", server: \"linear\", args: { redirectUrl: \"PASTE_REDIRECT_URL_HERE\" } })"
        ));

        // No parseable port ⇒ the last two lines are absent entirely.
        let without_port =
            format_manual_auth_instructions("linear", "https://auth.example.com/authorize");
        assert_eq!(
            get_redirect_port("https://auth.example.com/authorize"),
            None
        );
        assert!(!without_port.contains("local port"));
        assert!(without_port.ends_with("JSON-string args remain supported."));
        // A default-port redirect is normalised away by both `new URL().port` and `Url::port()`.
        assert_eq!(
            get_redirect_port("https://a.example/x?redirect_uri=http%3A%2F%2Flocalhost%2Fcb"),
            None
        );
    }

    // ---- MCP-162 · the single-shot auto-auth latch ----------------------------------------------------------

    /// A `client_credentials` server: the one grant type [`attempt_auto_auth`] step 4 lets through
    /// in a headless session.
    fn machine_oauth(url: &str) -> ServerEntry {
        ServerEntry {
            url: Some(url.to_string()),
            oauth: Some(OAuthSetting::Config(crate::config::OAuthConfig {
                grant_type: Some(OAuthGrantType::ClientCredentials),
                ..crate::config::OAuthConfig::default()
            })),
            ..ServerEntry::default()
        }
    }

    fn auto_auth_on(mut config: McpConfig) -> McpConfig {
        config.settings = Some(McpSettings {
            auto_auth: Some(true),
            ..McpSettings::default()
        });
        config
    }

    /// Upstream: "fails fast for non-ui browser auth when autoAuth is enabled".
    ///
    /// Step 4 of the ladder refuses **before** `authenticate` is ever called when there is no
    /// interactive surface and the grant type needs a browser — so the counter stays at zero and the
    /// model is told how to start the flow manually.
    #[tokio::test]
    async fn headless_browser_auth_fails_fast_without_calling_authenticate() {
        let config = auto_auth_on(config_with(&[(
            "linear",
            http("https://linear.example/mcp"),
        )]));
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_oauth("linear.example");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result = execute_call(
            &ctx,
            "issues",
            None,
            Some("linear"),
            &CancelToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            result.details.clone().unwrap()["error"],
            json!("auth_required")
        );
        assert_eq!(
            env.authenticate_calls.load(Ordering::SeqCst),
            0,
            "no browser, no attempt"
        );
        assert_eq!(text_of(&result), default_auth_required_message("linear"));
    }

    /// Upstream: "uses custom authRequiredMessage for non-ui autoAuth failures" — the configured
    /// template still wins over the step-4 default, because that default routes through
    /// [`get_auth_required_message`] rather than being returned directly.
    #[tokio::test]
    async fn a_configured_auth_required_message_wins_over_the_headless_default() {
        let mut config = auto_auth_on(config_with(&[(
            "linear",
            http("https://linear.example/mcp"),
        )]));
        config.settings = Some(McpSettings {
            auto_auth: Some(true),
            auth_required_message: Some("Ask an admin to authorise ${server}.".to_string()),
            ..McpSettings::default()
        });
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_oauth("linear.example");
        let (ctx, _) = ctx_with(config, &[], &[], env);
        let result = execute_call(
            &ctx,
            "issues",
            None,
            Some("linear"),
            &CancelToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(text_of(&result), "Ask an admin to authorise linear.");
    }

    /// A failing `client_credentials` auto-auth reaches `authenticate` exactly once and reports
    /// through [`get_auth_failed_message`].
    #[tokio::test]
    async fn a_failed_auto_auth_reports_the_failure_message() {
        let config = auto_auth_on(config_with(&[(
            "linear",
            machine_oauth("https://linear.example/mcp"),
        )]));
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_oauth("linear.example")
            .with_authenticate_failure("token exchange refused");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result = execute_call(
            &ctx,
            "issues",
            None,
            Some("linear"),
            &CancelToken::new(),
            None,
        )
        .await
        .unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("auth_required"));
        assert_eq!(details["server"], json!("linear"));
        assert_eq!(details["requestedTool"], json!("issues"));
        assert_eq!(env.authenticate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            text_of(&result),
            "OAuth authentication failed for \"linear\": token exchange refused. Run mcp({ action: \"auth-start\", server: \"linear\" }) to get a browser URL, or /mcp-auth linear in an interactive local session."
        );
    }

    /// **The latch, observed across two call sites.**
    ///
    /// `foo_bar_thing` matches the prefixes of both `foo_bar` and `foo`, so phase 4 visits two
    /// `needs-auth` candidates in descending-prefix-length order. The first claims the latch and
    /// authenticates; the second finds the latch already set and does **not** authenticate again.
    /// Without the latch this single tool call would open two browser flows.
    #[tokio::test]
    async fn the_latch_stops_a_second_auto_auth_in_the_same_call() {
        let config = auto_auth_on(config_with(&[
            ("foo", machine_oauth("https://foo.example/mcp")),
            ("foo_bar", machine_oauth("https://foo.example/bar")),
        ]));
        let env = FakeEnv::default()
            .with_connection("foo", ConnectionStatus::NeedsAuth)
            .with_connection("foo_bar", ConnectionStatus::NeedsAuth)
            .with_oauth("foo.example");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result = execute_call(&ctx, "foo_bar_thing", None, None, &CancelToken::new(), None)
            .await
            .unwrap();
        // Neither candidate ever connects, so the call ends unresolved…
        assert_eq!(
            result.details.clone().unwrap()["error"],
            json!("tool_not_found")
        );
        // …and `authenticate` ran ONCE across both of them.
        assert_eq!(
            env.authenticate_calls.load(Ordering::SeqCst),
            1,
            "the latch is single-shot"
        );
    }

    /// `autoAuth` is opt-in, not opt-out: unset means the ladder is never entered.
    #[tokio::test]
    async fn auto_auth_is_opt_in() {
        let config = config_with(&[("linear", machine_oauth("https://linear.example/mcp"))]);
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_oauth("linear.example");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result = execute_call(
            &ctx,
            "issues",
            None,
            Some("linear"),
            &CancelToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            result.details.clone().unwrap()["error"],
            json!("auth_required")
        );
        assert_eq!(env.authenticate_calls.load(Ordering::SeqCst), 0);
        assert_eq!(text_of(&result), default_auth_required_message("linear"));
    }

    // ---- MCP-161 · `executeConnect` reports as a LIST --------------------------------------------------------

    #[tokio::test]
    async fn a_successful_connect_reports_mode_list() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_connection("srv", ConnectionStatus::Connected);
        let (ctx, _) = ctx_with(config, &[], &[], env);
        let result = execute_connect(&ctx, "srv", &CancelToken::new())
            .await
            .unwrap();
        // `details.mode === "list"` — a successful connect renders as a listing.
        assert_eq!(result.details.clone().unwrap()["mode"], json!("list"));

        assert_eq!(
            execute_connect(&ctx, "missing", &CancelToken::new())
                .await
                .unwrap()
                .details
                .clone()
                .unwrap()["error"],
            json!("not_found")
        );
    }

    #[tokio::test]
    async fn connect_deletes_instructions_a_server_stopped_sending() {
        let config = config_with(&[("srv", stdio("a"))]);
        let env = FakeEnv::default().with_connection("srv", ConnectionStatus::Connected);
        let (ctx, _) = ctx_with(config, &[], &[("srv", "stale text")], env);
        assert_eq!(
            ctx.server_instructions("srv").as_deref(),
            Some("stale text")
        );
        // `ConnectOutcome::instructions` is `None`, which is `state.serverInstructions.delete(...)`.
        execute_connect(&ctx, "srv", &CancelToken::new())
            .await
            .unwrap();
        assert_eq!(ctx.server_instructions("srv"), None);
    }

    // ---- MCP-168 · `executeAuthComplete` ---------------------------------------------------------------------

    #[tokio::test]
    async fn auth_complete_closes_the_connection_and_clears_the_failure() {
        let config = config_with(&[("linear", http("https://linear.example/mcp"))]);
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::NeedsAuth)
            .with_failure("linear", 5);
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result = execute_auth_complete(
            &ctx,
            "linear",
            "http://localhost:8976/cb?code=x",
            &CancelToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            result.details.clone().unwrap()["authenticated"],
            json!(true)
        );
        assert_eq!(
            text_of(&result),
            "OAuth authentication successful for \"linear\". Run mcp({ connect: \"linear\" }) to connect with the new token."
        );
        assert_eq!(
            env.get_connection("linear"),
            None,
            "the connection is closed"
        );
        assert_eq!(
            env.failure_age_seconds("linear"),
            None,
            "the failure record is cleared"
        );
    }

    // ---- MCP-167 · `executeAuthStart` ------------------------------------------------------------------------

    #[tokio::test]
    async fn auth_start_rejects_non_oauth_servers_and_renders_instructions_otherwise() {
        let config = config_with(&[
            ("stdio_srv", stdio("a")),
            ("linear", http("https://linear.example/mcp")),
        ]);
        let env = FakeEnv::default().with_oauth("linear.example");
        let (ctx, _) = ctx_with(config, &[], &[], env);

        let rejected = execute_auth_start(&ctx, "stdio_srv", &CancelToken::new())
            .await
            .unwrap();
        assert_eq!(
            rejected.details.clone().unwrap()["error"],
            json!("oauth_not_supported")
        );
        assert_eq!(
            text_of(&rejected),
            "Server \"stdio_srv\" is not configured for OAuth over HTTP."
        );

        let started = execute_auth_start(&ctx, "linear", &CancelToken::new())
            .await
            .unwrap();
        let details = started.details.clone().unwrap();
        assert_eq!(
            details["authorizationUrl"],
            json!("https://auth.example.com/authorize")
        );
        assert!(text_of(&started).starts_with("MCP OAuth required for \"linear\"."));
    }
}
