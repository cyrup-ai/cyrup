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
pub mod config_value;
pub mod env;
pub mod env_keys;
pub mod error;
pub mod lock;
pub mod model;
pub mod policy;
pub mod settings;
pub mod trust;

#[cfg(test)]
pub(crate) mod test_util;

pub use auth::{
    AuthStatus, AuthStore, Credential, CredentialSource, ResolvedAuth, Stored, resolve_auth,
};
pub use config_value::{
    clear_config_value_cache, config_value_env_var_name, config_value_env_var_names,
    is_command_config_value, is_config_value_configured, missing_config_value_env_var_names,
    resolve_config_value, resolve_config_value_or_throw, resolve_config_value_uncached,
    resolve_headers, resolve_headers_or_throw,
};
pub use env::{CacheRetention, CliConfigOverrides, ConfigDirs, EnvVars};
pub use env_keys::{api_key_env_vars, find_env_keys, get_env_api_key};
pub use error::{AuthError, ConfigError, ScopedError};
pub use model::{
    CliModelResult, InitialModelResult, ModelCycler, ModelFile, ModelResolver, ParsedModel,
    ProviderConfig, RestoredModelResult, ScopedModel, build_fallback_model,
    default_model_per_provider, find_initial_model, load_custom_models, load_models_file,
    parse_thinking_level, resolve_cli_model, restore_model_from_session,
};
pub use policy::NetworkPolicy;
pub use settings::{
    CompactionSettings, DEFAULT_HTTP_IDLE_TIMEOUT_MS, DefaultProjectTrust, EffectiveSettings,
    FileSettingsStore, InMemorySettingsStore, PackageSource, RetrySettings, Settings,
    SettingsManager, SettingsScope, SettingsStore, deep_merge, migrate_settings,
    parse_http_idle_timeout_ms,
};
pub use trust::{
    AppMode, ExtensionTrust, ResourceKind, ResourceStage, TrustDecision, TrustEntry, TrustInputs,
    TrustOption, TrustOutcome, TrustStore, decide_trust, decide_trust_with_extension,
    format_project_trust_prompt, has_trust_requiring_resources, project_trust_parent_path,
    resource_stage, select_loaded, should_load, trust_options,
};
