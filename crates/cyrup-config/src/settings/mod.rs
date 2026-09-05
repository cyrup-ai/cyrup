//! Layered settings: global ◁ project ◁ CLI deep-merge with unknown-key preservation
//! (arch-07 §3.2/§3.3/§4.3, R-07-001/004/005).
//!
//! Settings are represented structurally as a JSON object map. This makes unknown-key
//! preservation (R-07-004) and per-key nested deep-merge (R-07-001) trivially correct, while
//! typed getters apply documented defaults in one place (mirrors Pi's `getX()` accessors).
//!
//! Split by concern: `types` holds the scope + value enums/structs, `layer` is the raw one-scope
//! `Settings` document, `migrate` ports `migrateSettings`, `merge` is `deep_merge`, `effective` is
//! the merged read-only view and its typed getters, `store` is the read/lock seam plus its two
//! implementations, and `manager` is the two-layer facade and its writers.
//!
//! Submodules are private; every item is re-exported here, so `cyrup_config::settings::X` stays
//! the one public path for all of them.

mod effective;
mod layer;
mod manager;
mod merge;
mod migrate;
mod store;
mod types;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests;

pub use effective::{DEFAULT_HTTP_IDLE_TIMEOUT_MS, EffectiveSettings, parse_http_idle_timeout_ms};
pub use layer::Settings;
pub use manager::SettingsManager;
pub use merge::deep_merge;
pub use migrate::migrate_settings;
pub use store::{FileSettingsStore, InMemorySettingsStore, SettingsStore};
pub use types::{
    BranchSummarySettings, CompactionSettings, DefaultProjectTrust, FullscreenExitOutput,
    FullscreenScrollbar, MermaidRenderingMode, PackageSource, ProviderRetrySettings, RetrySettings,
    SettingsScope, ThinkingBudgets, TuiMode, Warnings,
};
