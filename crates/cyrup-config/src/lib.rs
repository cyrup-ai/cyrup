//! cyrup-config — settings, trust, auth, model resolution (arch-07; conformance: func-07).
//!
//! Layered `settings.json` (global/project/CLI deep-merge), staged project trust, the JSON-only
//! `auth.json` credential store (the arch-01 `CredentialStore` backing), and model resolution.
//!
//! Scaffold stub.

/// Configuration/trust/auth error (arch-07 §8). Scaffold placeholder.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}
