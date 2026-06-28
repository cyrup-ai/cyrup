//! cyrup-config — settings, trust, auth, model resolution (arch-07; conformance: func-07).
//!
//! This crate owns how cyrup is configured before and during a run:
//!
//! - **Layered settings** ([`settings`]): `global ◁ project ◁ CLI` deep-merge with unknown-key
//!   preservation (R-07-001/004/005).
//! - **Project trust** ([`trust`]): the persisted `trust.json` store with ancestor matching, the
//!   staged pre-/post-trust resource split, and the pure trust decision (R-07-006…R-07-013).
//! - **Credential storage** ([`auth`]): the `auth.json`-backed credential store with serialized
//!   read-modify-write, OAuth refresh under the lock, `0600` perms, and the request-time
//!   precedence helper (R-07-014…R-07-017, R-07-030).
//! - **Model resolution** ([`model`]): pattern matching, the `:level` thinking shorthand,
//!   per-provider defaults, scoping + cycling, custom `models.json` (R-07-019…R-07-023).
//! - **Network policy** ([`policy`]): the offline / telemetry / update-check gate (R-07-024…027).
//!
//! `cyrup-config` performs **no network I/O**; it decides *whether* startup network ops are
//! permitted (DI-10).
#![forbid(unsafe_code)]

pub mod auth;
pub mod env;
pub mod error;
pub mod lock;
pub mod model;
pub mod policy;
pub mod settings;
pub mod trust;

#[cfg(test)]
pub(crate) mod test_util;

pub use auth::{
    AuthStore, Credential, CredentialSource, ResolvedAuth, Stored, resolve_auth,
};
pub use env::{CacheRetention, CliConfigOverrides, ConfigDirs, EnvVars};
pub use error::{AuthError, ConfigError, ScopedError};
pub use model::{
    load_custom_models, parse_thinking_level, ModelCycler, ModelResolver, ParsedModel, ScopedModel,
};
pub use policy::NetworkPolicy;
pub use settings::{
    deep_merge, DefaultProjectTrust, EffectiveSettings, FileSettingsStore, InMemorySettingsStore,
    Settings, SettingsManager, SettingsScope, SettingsStore,
};
pub use trust::{
    decide_trust, has_trust_requiring_resources, resource_stage, select_loaded, should_load,
    trust_options, AppMode, ResourceKind, ResourceStage, TrustDecision, TrustEntry, TrustInputs,
    TrustOption, TrustOutcome, TrustStore,
};
