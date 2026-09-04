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
//!
//! [`login_selector_rows`] is the newer, option-shaped builder the live `/login` and `/logout`
//! pickers use: it takes the resolved `AuthSelectorProvider[]`
//! (`cyrup_config::login::{login_provider_options, logout_provider_options}`) rather than raw
//! `(id, state)` pairs, so a provider offering BOTH a subscription and an API-key login gets the two
//! rows upstream gives it. [`provider_rows`] remains for the id-only shape.

use cyrup_config::login::{AuthType, LoginProviderOption};

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
    for (i, word) in id
        .split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .enumerate()
    {
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
    if out.is_empty() { id.to_string() } else { out }
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
    rows.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then(a.0.cmp(&b.0))
    });
    rows
}

/// `formatAuthSelectorProviderType` (`oauth-selector.ts:22-24`).
pub fn format_auth_selector_provider_type(auth_type: AuthType) -> &'static str {
    match auth_type {
        AuthType::Oauth => "subscription",
        AuthType::ApiKey => "API key",
    }
}

/// `/^[A-Z][A-Z0-9_]*(?:, [A-Z][A-Z0-9_]*)*$/` (`oauth-selector.ts:176`) — "does this source read
/// like a list of environment-variable names?", which is what makes the row say `✓ env: OPENAI_API_KEY`
/// instead of echoing the bare source. Hand-rolled rather than pulling in a regex crate for one
/// pattern (`cyrup/Cargo.toml:174-180` — prefer no new dependency where a small pure function does).
fn looks_like_env_var_list(source: &str) -> bool {
    !source.is_empty()
        && source.split(", ").all(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) if first.is_ascii_uppercase() => {
                    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                }
                _ => false,
            }
        })
}

/// The theme colour one run of the status indicator is painted in.
///
/// **S21.** `formatStatusIndicator` (`oauth-selector.ts:164-181`) does not return one uniformly
/// coloured string: `" ✓ configured"` is `theme.fg("success", …)` (`:175`), the mismatch case is
/// `theme.fg("muted", " • ") + theme.fg("warning", label)` (`:168`) — **two** runs — and
/// `" • unconfigured"` is `theme.fg("muted", …)` (`:165`). cyrup folded the whole thing into a
/// `SelectItem.description`, which `select_list.rs` paints uniformly `muted` (or, on the highlighted
/// row, uniformly `accent`), so a configured provider read grey instead of green and a credential
/// mismatch lost its warning entirely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    /// `theme.fg("muted", …)`.
    Muted,
    /// `theme.fg("warning", …)`.
    Warning,
    /// `theme.fg("success", …)`.
    Success,
}

/// `formatStatusIndicator(provider)` as the **styled runs** upstream actually emits
/// (`oauth-selector.ts:164-181`), each keeping its own colour — see [`StatusTone`].
///
/// The text is verbatim upstream's, **leading space included**: the indicator is concatenated
/// straight onto the provider name at `:138`/`:141` (`prefix + text + authTypeLabel +
/// statusIndicator`), so the single space in `" ✓ configured"` is the entire gap between the name
/// and its status. [`format_status_indicator`] keeps returning the space-less form, because that one
/// feeds a padded description column rather than this concatenation.
pub fn status_indicator_runs(option: &LoginProviderOption) -> Vec<(StatusTone, String)> {
    // `if (!provider.status) return theme.fg("muted", " • unconfigured")` (`:165`).
    let Some(status) = option.status.as_ref() else {
        return vec![(StatusTone::Muted, " • unconfigured".to_string())];
    };
    // `:166-169` — a stored credential of the OTHER kind: muted bullet, warning label.
    if status.auth_type != option.auth_type {
        let label = match status.auth_type {
            AuthType::Oauth => "subscription configured",
            AuthType::ApiKey => "API key configured",
        };
        return vec![
            (StatusTone::Muted, " • ".to_string()),
            (StatusTone::Warning, label.to_string()),
        ];
    }
    // `:170-176`.
    let source = match status.source.as_deref() {
        None | Some("") | Some("OAuth") | Some("stored credential") => {
            return vec![(StatusTone::Success, " ✓ configured".to_string())];
        }
        Some(source) => source,
    };
    if looks_like_env_var_list(source) {
        vec![(StatusTone::Success, format!(" ✓ env: {source}"))]
    } else {
        vec![(StatusTone::Success, format!(" ✓ {source}"))]
    }
}

/// `formatStatusIndicator(provider)` (`oauth-selector.ts:164-181`) — the row's trailing status,
/// verbatim including the mismatch case (a provider whose STORED credential is of the other kind
/// shows `• subscription configured` / `• API key configured`, not `✓ configured`).
pub fn format_status_indicator(option: &LoginProviderOption) -> String {
    // `if (!provider.status) return " • unconfigured"` (`:165`).
    let Some(status) = option.status.as_ref() else {
        return "• unconfigured".to_string();
    };
    // `if (provider.status.type !== provider.authType)` (`:166-169`).
    if status.auth_type != option.auth_type {
        let label = match status.auth_type {
            AuthType::Oauth => "subscription configured",
            AuthType::ApiKey => "API key configured",
        };
        return format!("• {label}");
    }
    // `if (!source || source === "OAuth" || source === "stored credential")` (`:170-176`).
    let source = match status.source.as_deref() {
        None | Some("") | Some("OAuth") | Some("stored credential") => {
            return "✓ configured".to_string();
        }
        Some(source) => source,
    };
    if looks_like_env_var_list(source) {
        format!("✓ env: {source}")
    } else {
        format!("✓ {source}")
    }
}

/// The `OAuthSelectorComponent` rows for a resolved `AuthSelectorProvider[]`
/// (`oauth-selector.ts:124-145`), as `(value, label, description)`.
///
/// * **value** is the row's INDEX into `options`. Pi calls back with `(providerId, authType)` and
///   re-finds the option (`interactive-mode.ts:5106-5111`); cyrup's selector carries a single
///   string, and the index is the only key that survives one provider contributing two rows.
/// * **label** is `` `${name}${authTypeLabel}` ``, where the ` [subscription]` / ` [API key]` suffix
///   appears only when the list MIXES both kinds (`showAuthTypeLabels`, `oauth-selector.ts:61`).
/// * **description** is [`format_status_indicator`].
///
/// `options` is already sorted by display name by `login_provider_options`/`logout_provider_options`
/// (Node `localeCompare`, via `feruca`), so this preserves their order.
pub fn login_selector_rows(
    options: &[LoginProviderOption],
) -> Vec<(String, String, Option<String>)> {
    // `new Set(providers.map(p => p.authType)).size > 1` (`oauth-selector.ts:61`).
    let show_auth_type_labels = options
        .iter()
        .any(|o| o.auth_type != options.first().map_or(o.auth_type, |f| f.auth_type));
    options
        .iter()
        .enumerate()
        .map(|(i, option)| {
            let label = if show_auth_type_labels {
                format!(
                    "{} [{}]",
                    option.name,
                    format_auth_selector_provider_type(option.auth_type)
                )
            } else {
                option.name.clone()
            };
            (i.to_string(), label, Some(format_status_indicator(option)))
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn auth_state_maps_store_status() {
        assert_eq!(AuthState::from_status(true, false), AuthState::Configured);
        assert_eq!(
            AuthState::from_status(false, true),
            AuthState::EnvConfigured
        );
        assert_eq!(
            AuthState::from_status(false, false),
            AuthState::Unconfigured
        );
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
        assert_eq!(
            provider_display_name("vercel-ai-gateway"),
            "Vercel AI Gateway"
        );
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
