//! Loads the SubAgents extension's [`SubagentExtensionConfig`] (arch-SA §4.6/§6.1;
//! `cyrup_ext_subagents::registration`'s R-SA-133 five-tier precedence) for the one binary-level
//! tier this crate is actually responsible for supplying: **tier 3, `config.json`** — the
//! per-installation extension config file, read from `<agent_dir>/subagents/config.json`.
//!
//! The other four tiers of R-SA-133 are NOT this module's concern: tier 1 (inline per-call
//! overrides) and tier 2 (`subagents.*` `cyrup-config` settings) are resolved per-call by
//! `cyrup_ext_subagents::extension::SubagentExecutor` itself (it already holds a live
//! `SubagentExtensionConfig` snapshot to layer under those, via
//! [`cyrup_ext_subagents::extension::SubagentsExtension::with_config`]); tier 4 (agent
//! frontmatter defaults) is per-agent and resolved at discovery time; tier 5 (hardcoded defaults)
//! is [`SubagentExtensionConfig::default`] itself, applied automatically by
//! [`serde`]'s `#[serde(default)]` struct-level attribute when a partial (or absent) `config.json`
//! is read.
//!
//! A missing `config.json` is normal (most installations never create one) and is NOT an error —
//! it yields [`SubagentExtensionConfig::default`] directly, matching every other config-loading
//! seam in this binary's own convention of "absent config is the all-defaults case, not a failure"
//! (mirrors `cyrup_config::SettingsManager::load`'s own tolerant-of-absence behavior). A `config.json`
//! that EXISTS but fails to parse as valid JSON IS surfaced as a warning on stderr (never silently
//! swallowed, so a hand-edited typo is discoverable) and this function still falls back to the
//! default rather than aborting startup over one malformed optional file.

use std::path::Path;

use cyrup_ext_subagents::registration::SubagentExtensionConfig;

/// Load the SubAgents extension's `config.json` (R-SA-133 tier 3) from
/// `<agent_dir>/subagents/config.json`, or fall back to
/// [`SubagentExtensionConfig::default`] (tier 5) when the file is absent or fails to parse.
#[must_use]
pub fn load_subagent_extension_config(agent_dir: &Path) -> SubagentExtensionConfig {
    let path = agent_dir.join("subagents").join("config.json");
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<SubagentExtensionConfig>(&bytes) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!(
                    "cyrup: warning: {} is not valid subagents config JSON ({err}); using defaults",
                    path.display()
                );
                SubagentExtensionConfig::default()
            }
        },
        Err(_) => SubagentExtensionConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn absent_config_json_yields_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = load_subagent_extension_config(dir.path());
        assert_eq!(cfg, SubagentExtensionConfig::default());
    }

    #[test]
    fn malformed_config_json_falls_back_to_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(subagents_dir.join("config.json"), "not json at all").expect("write");
        let cfg = load_subagent_extension_config(dir.path());
        assert_eq!(cfg, SubagentExtensionConfig::default());
    }

    #[test]
    fn valid_partial_config_json_overrides_only_the_present_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"maxSubagentDepth": 5}"#,
        )
        .expect("write");
        let cfg = load_subagent_extension_config(dir.path());
        assert_eq!(cfg.max_subagent_depth, 5);
        assert_eq!(
            cfg.global_concurrency_limit,
            SubagentExtensionConfig::default().global_concurrency_limit
        );
    }
}
