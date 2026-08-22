//! Provider login guidance strings — a 1:1 port of pi `packages/coding-agent/src/core/
//! auth-guidance.ts` @v0.83.0.
//!
//! These live at the `core/` tier in pi (next to `sdk.ts` and `agent-session.ts`), which is exactly
//! this crate, because BOTH tiers consume them: `sdk.ts:216-218` turns the modelless case into a
//! `modelFallbackMessage` **banner**, and `agent-session.ts:1178-1180` throws
//! `formatNoModelSelectedMessage()` when a prompt is attempted with no model selected. The bin tier
//! (`main.ts:852-855`) imports the same module for its non-interactive hard stop.
//!
//! The doc paths are shown relative to the package docs dir, matching
//! [`crate::auth_guidance::get_provider_login_help`]'s counterpart in `cyrup::diagnostics` (the
//! absolute prefix pi derives from `getDocsPath()` is environment-cosmetic).

/// Pi `getProviderLoginHelp` (auth-guidance.ts:6-11).
pub(crate) fn get_provider_login_help() -> String {
    "Use /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md"
        .to_string()
}

/// Pi `formatNoModelsAvailableMessage` (auth-guidance.ts:14-16).
///
/// This is the `modelFallbackMessage` a modelless session carries (sdk.ts:216-218) — a WARNING the
/// interactive front-end shows (interactive-mode.ts:883-884), and the stderr text the bin prints
/// before `exit(1)` in every NON-interactive mode (main.ts:852-855).
pub(crate) fn format_no_models_available_message() -> String {
    format!("No models available. {}", get_provider_login_help())
}

/// Pi `formatNoModelSelectedMessage` (auth-guidance.ts:18-20).
///
/// Thrown by `prompt`/`compact` when the session has no model (agent-session.ts:1178-1180,
/// :1790-1792) — the error a first-run user sees if they type before running `/login` + `/model`.
pub(crate) fn format_no_model_selected_message() -> String {
    format!("No model selected.\n\n{}\n\nThen use /model to select a model.", get_provider_login_help())
}

/// pi `UNKNOWN_PROVIDER` (auth-guidance.ts:4). A model whose provider could not be identified is
/// named "the selected model" rather than the literal string `unknown`.
pub(crate) const UNKNOWN_PROVIDER: &str = "unknown";

/// Pi `formatNoApiKeyFoundMessage` (auth-guidance.ts:22-25). PROV-037.
///
/// The message a submit-time preflight refuses with when the selected model's provider has no
/// resolvable credential and is NOT OAuth-backed. Thrown from three upstream sites, all in
/// `core/agent-session.ts` @v0.83.0: `:418` (the resolver reported "authHeader requires a resolved
/// API key"), `:438` (auth resolved to nothing at all) and `:1194` (the pre-send preflight).
///
/// cyrup previously answered all of these with its own `no configured auth for model: p/m`, which
/// named no remedy — `grep -rn 'No API key found' crates/` returned zero.
pub(crate) fn format_no_api_key_found_message(provider: &str) -> String {
    let provider_display = if provider == UNKNOWN_PROVIDER { "the selected model" } else { provider };
    format!("No API key found for {provider_display}.\n\n{}", get_provider_login_help())
}

/// The OAuth-expiry variant of the same refusal — pi's inline template at
/// `core/agent-session.ts:1188-1192` (and the byte-identical copy at `:432-436`). PROV-037.
///
/// It exists because an expired OAuth token and a missing API key are DIFFERENT user problems with
/// different fixes: this one names the provider, distinguishes expiry from a network outage, and
/// tells the user the exact command to run. Not a function upstream — the string is built inline at
/// both sites — but it is built identically at both, so it is factored here rather than duplicated.
pub(crate) fn format_oauth_reauthenticate_message(provider: &str) -> String {
    format!(
        "Authentication failed for \"{provider}\". Credentials may have expired or network is \
         unavailable. Run '/login {provider}' to re-authenticate."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_match_pi_auth_guidance() {
        // pi auth-guidance.ts:14-16 — `No models available. ${getProviderLoginHelp()}`.
        assert_eq!(
            format_no_models_available_message(),
            "No models available. Use /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md"
        );
        // pi auth-guidance.ts:18-20 — the `/login` … `/model` two-step a modelless first run needs.
        assert_eq!(
            format_no_model_selected_message(),
            "No model selected.\n\nUse /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md\n\nThen use /model to select a model."
        );
    }

    /// PROV-037 — the two refusal strings a credential-less submit produces.
    ///
    /// **Red before the fix:** `format_no_api_key_found_message` did not exist
    /// (`grep -rn 'No API key found' crates/` returned zero, and so did
    /// `grep -rn 'Authentication failed for' crates/`), so this did not compile. The user got
    /// cyrup's `no configured auth for model: p/m`, which names no provider-specific remedy and no
    /// `/login` command.
    #[test]
    fn prov037_refusal_messages_are_byte_identical_to_pi() {
        // auth-guidance.ts:22-25.
        assert_eq!(
            format_no_api_key_found_message("anthropic"),
            "No API key found for anthropic.\n\nUse /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md"
        );
        // The `UNKNOWN_PROVIDER` carve-out at `:23` — the one branch a naive port drops, which
        // would print the literal word "unknown" at the user.
        assert_eq!(
            format_no_api_key_found_message(UNKNOWN_PROVIDER),
            "No API key found for the selected model.\n\nUse /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md"
        );
        // agent-session.ts:1188-1192 — note the DOUBLE QUOTES around the provider and the single
        // quotes around the command; both are upstream's.
        assert_eq!(
            format_oauth_reauthenticate_message("openai-codex"),
            "Authentication failed for \"openai-codex\". Credentials may have expired or network is unavailable. Run '/login openai-codex' to re-authenticate."
        );
    }

    /// PROV-037 — the preflight's error variant must render pi's message and NOTHING else.
    ///
    /// The sibling `NoConfiguredAuth` prefixes its payload with `no configured auth for model: `,
    /// which is right for that variant and wrong for this one: upstream `throw new Error(msg)`
    /// surfaces `msg` verbatim, so any prefix here is text pi never shows. Pinning it keeps a later
    /// edit to `error.rs` from quietly reintroducing one.
    ///
    /// **Red before the fix:** `SessionServiceError::AuthPreflightRefused` did not exist.
    #[test]
    fn prov037_preflight_error_renders_pi_text_with_no_prefix() {
        let msg = format_no_api_key_found_message("groq");
        let err = crate::error::SessionServiceError::AuthPreflightRefused(msg.clone());
        assert_eq!(err.to_string(), msg, "the Display must be `{{0}}`, not a labelled variant");

        // And the sibling variant deliberately still carries cyrup's own label.
        let other = crate::error::SessionServiceError::NoConfiguredAuth("groq/x".to_string());
        assert_eq!(other.to_string(), "no configured auth for model: groq/x");
    }
}
