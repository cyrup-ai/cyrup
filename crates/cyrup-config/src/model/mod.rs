//! Model resolution: pattern matching (`provider/id`, bare id, partial/alias), the `:level`
//! thinking shorthand, per-provider defaults, scoping + cycling, and custom `models.json`
//! (arch-07 §3.6/§6.4, R-07-019…R-07-023).
//!
//! Split by concern: `resolver` matches patterns and expands `--models` scope, `glob` is the
//! self-contained minimatch engine it filters with, `cycler` walks the scoped set, `defaults`
//! holds the curated per-provider table, `select` is the CLI/initial/restore decision layer, and
//! `schema` / `load` / `validate` / `compose` are the `models.json` pipeline: types, read,
//! judge, apply.
//!
//! Submodules are private; every item is re-exported here, so `cyrup_config::model::X` stays the
//! one public path for all of them.

mod compose;
mod cycler;
mod defaults;
mod glob;
mod load;
mod resolver;
mod schema;
mod select;
mod validate;

#[cfg(test)]
mod fixtures;

pub(crate) use compose::apply_models_json;
pub use compose::{models_json_provider_is_configured, provider_is_configured};
pub use cycler::ModelCycler;
pub use defaults::{build_fallback_model, default_model_per_provider};
pub use load::{load_custom_models, load_models_file, load_models_file_reporting};
pub use resolver::{
    ModelResolver, ModelScopeDiagnostic, ModelScopeDiagnosticCode, ModelScopeDiagnosticLevel,
    ModelScopeResult, ParsedModel, ScopedModel, parse_thinking_level,
};
pub use schema::{
    ModelCostOverride, ModelDefinition, ModelFile, ModelOverride, ModelsJsonOauth, ProviderConfig,
    ResolvedRequestAuth,
};
pub use select::{
    CliModelResult, InitialModelResult, RestoredModelResult, find_initial_model, resolve_cli_model,
    restore_model_from_session,
};
pub use validate::{ModelsSchemaError, validate_models_config};
