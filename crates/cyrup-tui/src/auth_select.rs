//! Login / logout (oauth) provider-selector row sourcing (spec/tui/05 §6 "data-bound selectors";
//! port of Pi's `oauth-selector.ts` + `getLoginProviderOptions`/`getLogoutProviderOptions`,
//! `interactive-mode.ts:4594-4636`).
//!
//! Pi opens `/login` and `/logout` through the same editor-swap selector path as every other family:
//! the provider list is sourced from the session's auth store + model registry, sorted by display
//! name, each row carrying a status line. The *credential write* differs by mode — `/logout` deletes
//! the stored credential (a real, in-crate effect against [`cyrup_config::AuthStore`]); `/login`'s
//! device/PKCE-or-api-key dialog is the provider-tail residual, so the picker + status UI is built
//! here and confirming surfaces the chosen provider's next step.
//!
//! This module is the **pure** row-builder half (no session, no I/O) so it is unit-testable in
//! isolation; `app::execute_command` gathers the raw inputs (stored ids, catalog provider ids, per-
//! provider auth state) from the live `AgentSession` and calls [`provider_rows`].

/// The three auth states the oauth-selector status line distinguishes (Pi `getStatusText`,
/// `oauth-selector.ts:151-159`). Kept independent of the `cyrup-config` `AuthStatus` type so the row
/// builder stays a pure function — `app::execute_command` maps the live `AuthStatus` onto this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthState {
    /// A credential is **stored** for this provider (Pi `✓ configured`, the green line).
    Configured,
    /// No stored credential, but an environment var / `--api-key` runtime key is present
    /// (Pi api-key `configured via env`, the muted line).
    EnvConfigured,
    /// Nothing configured for this provider (Pi `• unconfigured`).
    Unconfigured,
}

impl AuthState {
    /// Map the live store's `(configured, has_source)` onto the selector's three-state status. A
    /// stored credential reports `configured = true`; an env/runtime key reports `configured = false`
    /// with `source = Some(_)`; nothing configured reports `source = None`
    /// (`auth.rs::get_auth_status`).
    pub fn from_status(configured: bool, has_source: bool) -> Self {
        if configured {
            AuthState::Configured
        } else if has_source {
            AuthState::EnvConfigured
        } else {
            AuthState::Unconfigured
        }
    }

    /// The row's right-column status text (Pi `getStatusText`, `oauth-selector.ts:153-158`).
    pub fn status_text(self) -> &'static str {
        match self {
            AuthState::Configured => "✓ configured",
            AuthState::EnvConfigured => "configured via env",
            AuthState::Unconfigured => "• unconfigured",
        }
    }
}

/// A faithful provider **display name** from its id (Pi `getProviderDisplayName` fallback,
/// model-registry.ts:787-793 — when no registered/oauth name is known it title-cases the id). Splits
/// on `-`/`_`/space, upper-cases each word's first character, and special-cases the common acronyms so
/// `openai` → `Openai`-but-`OpenAI`, `xai` → `xAI`, `ai`/`api` stay upper. Never panics.
pub fn provider_display_name(id: &str) -> String {
    // A handful of well-known ids whose canonical casing the title-caser cannot derive.
    match id {
        "openai" => return "OpenAI".to_string(),
        "openai-codex" => return "OpenAI Codex".to_string(),
        "xai" => return "xAI".to_string(),
        "github-copilot" => return "GitHub Copilot".to_string(),
        "google-vertex" => return "Google Vertex".to_string(),
        "azure-openai-responses" => return "Azure OpenAI".to_string(),
        "zai" => return "Z.AI".to_string(),
        _ => {}
    }
    let mut out = String::with_capacity(id.len());
    for (i, word) in id.split(['-', '_', ' ']).filter(|w| !w.is_empty()).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let upper = matches!(word, "ai" | "api" | "cn" | "go");
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            if upper {
                out.extend(word.chars().flat_map(char::to_uppercase));
            } else {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    if out.is_empty() {
        id.to_string()
    } else {
        out
    }
}

/// Build the selector rows from `(provider_id, state)` entries (Pi `getLoginProviderOptions`/
/// `getLogoutProviderOptions` → `AuthSelectorProvider[]` sorted by `name`). Each row is
/// `(value = provider id, label = display name, description = status text)`, sorted by display name
/// (locale-insensitive — Pi's `localeCompare`). Used for both `/login` (catalog providers) and
/// `/logout` (stored providers).
pub fn provider_rows(entries: Vec<(String, AuthState)>) -> Vec<(String, String, Option<String>)> {
    let mut rows: Vec<(String, String, Option<String>)> = entries
        .into_iter()
        .map(|(id, state)| {
            let name = provider_display_name(&id);
            (id, name, Some(state.status_text().to_string()))
        })
        .collect();
    rows.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then(a.0.cmp(&b.0)));
    rows
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn auth_state_maps_store_status() {
        assert_eq!(AuthState::from_status(true, false), AuthState::Configured);
        assert_eq!(AuthState::from_status(false, true), AuthState::EnvConfigured);
        assert_eq!(AuthState::from_status(false, false), AuthState::Unconfigured);
        // `configured` wins even if a source is also present (stored beats env).
        assert_eq!(AuthState::from_status(true, true), AuthState::Configured);
    }

    #[test]
    fn status_text_matches_pi_lines() {
        assert_eq!(AuthState::Configured.status_text(), "✓ configured");
        assert_eq!(AuthState::EnvConfigured.status_text(), "configured via env");
        assert_eq!(AuthState::Unconfigured.status_text(), "• unconfigured");
    }

    #[test]
    fn display_name_title_cases_and_special_cases() {
        assert_eq!(provider_display_name("anthropic"), "Anthropic");
        assert_eq!(provider_display_name("openai"), "OpenAI");
        assert_eq!(provider_display_name("xai"), "xAI");
        assert_eq!(provider_display_name("github-copilot"), "GitHub Copilot");
        assert_eq!(provider_display_name("vercel-ai-gateway"), "Vercel AI Gateway");
        assert_eq!(provider_display_name("moonshotai-cn"), "Moonshotai CN");
        assert_eq!(provider_display_name(""), "");
    }

    #[test]
    fn rows_sort_by_display_name_and_carry_status() {
        let rows = provider_rows(vec![
            ("openai".to_string(), AuthState::Unconfigured),
            ("anthropic".to_string(), AuthState::Configured),
        ]);
        // Sorted by display name: Anthropic < OpenAI.
        assert_eq!(rows[0].0, "anthropic");
        assert_eq!(rows[0].1, "Anthropic");
        assert_eq!(rows[0].2.as_deref(), Some("✓ configured"));
        assert_eq!(rows[1].0, "openai");
        assert_eq!(rows[1].2.as_deref(), Some("• unconfigured"));
    }
}
