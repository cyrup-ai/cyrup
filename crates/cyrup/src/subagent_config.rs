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
    let Ok(bytes) = std::fs::read(&path) else {
        return SubagentExtensionConfig::default();
    };
    // pi `readConfigForUpdate` (`pi-subagents/src/extension/config.ts:15-28`) runs
    // `validateMissionStoreConfig(config.missions)` on the RAW parsed JSON before the typed view
    // is taken, because serde/`ExtensionConfig` field matching alone would silently DROP an
    // unknown key inside the `missions` block rather than refuse it. Upstream throws; this
    // loader's own established convention for a bad-but-present config file is warn-and-default
    // (see the module docs), so that is what a refused `missions` block gets too.
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(raw) => {
            if let Err(message) = SubagentExtensionConfig::validate_missions(&raw) {
                eprintln!(
                    "cyrup: warning: {} has an invalid missions block ({message}); using defaults",
                    path.display()
                );
                return SubagentExtensionConfig::default();
            }
        }
        Err(_) => {
            // Not valid JSON at all — the typed parse below reports it with the existing message.
        }
    }
    match serde_json::from_slice::<SubagentExtensionConfig>(&bytes) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!(
                "cyrup: warning: {} is not valid subagents config JSON ({err}); using defaults",
                path.display()
            );
            SubagentExtensionConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

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
    fn an_unknown_key_inside_the_missions_block_is_refused_and_defaulted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"maxSubagentDepth": 5, "missions": {"enabled": true, "nope": 1}}"#,
        )
        .expect("write");
        // pi `validateMissionStoreConfig` refuses the whole block; this loader's warn-and-default
        // convention then discards the file rather than honoring a half-understood config.
        assert_eq!(
            load_subagent_extension_config(dir.path()),
            SubagentExtensionConfig::default()
        );
    }

    #[test]
    fn a_valid_missions_block_is_loaded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"missions": {"enabled": false, "retainTerminal": 12}}"#,
        )
        .expect("write");
        let cfg = load_subagent_extension_config(dir.path());
        let missions = cfg.missions.expect("missions block");
        assert_eq!(missions.enabled, Some(false));
        assert_eq!(missions.retain_terminal, Some(12));
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

    /// A non-positive tuning knob must NOT take the rest of the file down with it.
    ///
    /// Upstream's `positiveInteger` (`proactive-skills.ts:32-36`) returns `undefined` for a value
    /// below 1 and the caller falls back to its default, leaving every other setting intact. cyrup
    /// typed `minReferences`/`maxRecommendations` as `u32`, so serde failed on `-1` before the
    /// guard ran — and because this loader discards the whole document on any deserialization
    /// error, a single bad knob silently reset `maxSubagentDepth`, `globalConcurrencyLimit`,
    /// `parallel.maxTasks`, every `control.*` key and the rest of the file to defaults, with
    /// nothing but an eprintln to say so.
    #[test]
    fn a_non_positive_tuning_knob_does_not_discard_the_rest_of_the_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"maxSubagentDepth": 5, "globalConcurrencyLimit": 9,
                "proactiveSkillSubagents": {"minReferences": -1, "maxRecommendations": 0}}"#,
        )
        .expect("write");

        let cfg = load_subagent_extension_config(dir.path());

        // The unrelated settings survive — this is the whole point.
        assert_eq!(
            cfg.max_subagent_depth, 5,
            "a bad proactive knob must not reset an unrelated setting"
        );
        assert_eq!(
            cfg.global_concurrency_limit, 9,
            "a bad proactive knob must not reset an unrelated setting"
        );

        // The out-of-range values themselves reach the guard rather than the parser.
        let Some(cyrup_ext_subagents::registration::ProactiveSkillSubagents::Config(p)) =
            cfg.proactive_skill_subagents.as_ref()
        else {
            panic!("the proactive block itself must survive: {cfg:?}");
        };
        assert_eq!(p.min_references, Some(-1));
        assert_eq!(p.max_recommendations, Some(0));
    }
}
