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
pub fn get_provider_login_help() -> String {
    "Use /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md"
        .to_string()
}

/// Pi `formatNoModelsAvailableMessage` (auth-guidance.ts:14-16).
///
/// This is the `modelFallbackMessage` a modelless session carries (sdk.ts:216-218) — a WARNING the
/// interactive front-end shows (interactive-mode.ts:883-884), and the stderr text the bin prints
/// before `exit(1)` in every NON-interactive mode (main.ts:852-855).
pub fn format_no_models_available_message() -> String {
    format!("No models available. {}", get_provider_login_help())
}

/// Pi `formatNoModelSelectedMessage` (auth-guidance.ts:18-20).
///
/// Thrown by `prompt`/`compact` when the session has no model (agent-session.ts:1178-1180,
/// :1790-1792) — the error a first-run user sees if they type before running `/login` + `/model`.
pub fn format_no_model_selected_message() -> String {
    format!("No model selected.\n\n{}\n\nThen use /model to select a model.", get_provider_login_help())
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
}
