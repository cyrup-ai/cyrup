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
//!
//! # SUBA-166 — except when the file declares a policy
//!
//! Warn-and-default is the wrong answer for a file that sets a POLICY key. A typo in `artifactDir`
//! in a `config.json` that also carries `{"authorityPolicy": {"stopRun": "forbid"}}` or
//! `permissions` rules used to discard both and run the session with no restrictions at all, with a
//! single stderr line as the only trace. pi fixed the same fail-open in `9f1c2552` (#2624): its
//! `loadConfig` (`pi-subagents/src/extension/config.ts:226-240` @v0.75.0) catches the validation
//! failure, re-reads the file and RETHROWS when it holds any
//! [`cyrup_ext_subagents::registration::FAIL_CLOSED_CONFIG_KEYS`] key, so only a file declaring
//! none of them falls back to `{}`. This loader refuses the same files, through
//! [`SubagentExtensionConfig::invalid_config_disposition`].

use cyrup_config::ConfigDirs;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::{InvalidConfigDisposition, SubagentExtensionConfig};
use std::path::{Path, PathBuf};

/// SUBA-166 — a `config.json` that exists, failed validation, and declares at least one
/// [`cyrup_ext_subagents::registration::FAIL_CLOSED_CONFIG_KEYS`] key.
///
/// Returned instead of the all-defaults config so the extension is never built with the operator's
/// declared `authorityPolicy`, `permissions` or route identity replaced by the built-in defaults.
///
/// The blast radius is pi's: `crate::session_launch::attach_native_extensions` QUARANTINES the one
/// extension (`crate::session_launch`'s `quarantine`), exactly as pi's loader `load.discard()`s the
/// factory that threw and records `Failed to load extension: <message>`
/// (`pi/packages/coding-agent/src/core/extensions/loader.ts:613-630`, `:655` @v1.0.1). The
/// session is built with the subagents extension absent, the refusal is reported as a fatal
/// extension-load diagnostic, and the other native built-ins still attach. It used to leave the
/// launch path instead, which aborted the whole launch over a hand-edited typo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedSubagentConfig {
    /// The `config.json` that was refused.
    path: PathBuf,
    /// The validator's (or serde's) own message — upstream rethrows this very error.
    message: String,
    /// The fail-closed keys the file declares, in upstream's list order.
    keys: Vec<&'static str>,
}

impl std::fmt::Display for RefusedSubagentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is invalid ({}) and sets {}: a declared policy must not be silently discarded and replaced by the built-in defaults",
            self.path.display(),
            self.message,
            self.keys.join(", "),
        )
    }
}

impl std::error::Error for RefusedSubagentConfig {}

impl RefusedSubagentConfig {
    /// The fail-closed keys that forced the refusal, in upstream's list order.
    #[must_use]
    pub fn keys(&self) -> &[&'static str] {
        &self.keys
    }
}

/// Load the SubAgents extension's `config.json` (R-SA-133 tier 3) from
/// `<dirs.agent_dir>/subagents/config.json`, or fall back to
/// [`SubagentExtensionConfig::default`] (tier 5) when the file is absent or fails to parse.
///
/// # This is where the extension's roots are decided
///
/// Takes the whole [`ConfigDirs`] rather than just the agent dir because it sets
/// `SubagentExtensionConfig::roots` on every return path, anchored on the layout the binary has
/// already resolved. That is the point of the field: one resolution, in the startup phase that
/// owns layout, instead of four resolvers re-deriving it from the environment deep inside the
/// crate — where they could, and did, answer differently from each other.
///
/// # Errors
///
/// SUBA-166 — [`RefusedSubagentConfig`] when the file exists, fails validation or the typed parse,
/// and declares a [`cyrup_ext_subagents::registration::FAIL_CLOSED_CONFIG_KEYS`] key. Every other
/// bad-but-present file still warns on stderr and yields the defaults.
///
/// The caller ACTS on this error rather than aborting over it — it quarantines the one extension —
/// which is why the refusal is a `Result` here and not a third variant of the returned config
/// (`docs/RUST-DESIGN-REVIEW.md`: expected domain outcomes are enum variants, technical failures
/// that abort the operation are `Result`; this one aborts building THIS extension and nothing
/// else). `crate::session_launch`'s `SubagentAttachment` is where the three outcomes the launch
/// path distinguishes are named.
pub fn load_subagent_extension_config(
    dirs: &ConfigDirs,
) -> Result<SubagentExtensionConfig, RefusedSubagentConfig> {
    // Every defaulting `return` below goes through this, so a config file that is absent,
    // unparseable or missions-invalid still gets the SAME roots a good one would.
    let rooted = || SubagentExtensionConfig {
        roots: Roots::from_config_dirs(dirs),
        ..SubagentExtensionConfig::default()
    };
    let path = dirs.agent_dir.join("subagents").join("config.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(rooted());
    };
    // pi `readConfigForUpdate` (`pi-subagents/src/extension/config.ts:15-28`) runs
    // `validateMissionStoreConfig(config.missions)` on the RAW parsed JSON before the typed view
    // is taken, because serde/`ExtensionConfig` field matching alone would silently DROP an
    // unknown key inside the `missions` block rather than refuse it. Upstream throws; this
    // loader's own established convention for a bad-but-present config file is warn-and-default
    // (see the module docs), so that is what a refused `missions` block gets too — unless the file
    // declares a fail-closed key, in which case upstream's throw is the only safe answer and
    // `refuse` produces it (SUBA-166).
    //
    // The same holds for every other raw validator pi's `validateConfig` runs
    // (`extension/config.ts:131-181` @v0.68.0): `artifactDir`, `authorityPolicy` and
    // `artifactConfig` were written and documented as refused at load, and never called here, so
    // a typo'd `authorityPolicy` action was ignored without a word. `validate_raw_config` runs
    // them all, in upstream's order.
    //
    // Keys this port does not read at all get a non-fatal warning each (`config_warnings`): the
    // rest of the file still loads, but nothing the user set disappears silently.
    let raw = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
    if let Some(raw) = raw.as_ref() {
        if let Err(message) = SubagentExtensionConfig::validate_raw_config(raw) {
            return match SubagentExtensionConfig::invalid_config_disposition(raw) {
                InvalidConfigDisposition::Refuse(keys) => Err(RefusedSubagentConfig {
                    path,
                    message,
                    keys,
                }),
                InvalidConfigDisposition::DefaultWithWarning => {
                    warn_and_default(&path, &format!("is invalid ({message})"), rooted)
                }
            };
        }
        for warning in SubagentExtensionConfig::config_warnings(raw) {
            eprintln!("cyrup: warning: {}: {warning}", path.display());
        }
    }
    // A `raw` of `None` is a file that is not valid JSON at all — the typed parse below reports it
    // with the existing message, and declares no key to fail closed on (upstream's re-read parse
    // fails the same way and its `readError === error` arm falls through to the log, `:235-237`).
    match serde_json::from_slice::<SubagentExtensionConfig>(&bytes) {
        // `roots` is `#[serde(skip)]`, so a parsed config carries `Default`'s env-derived value.
        // Overwrite it with the binary's own layout: a `config.json` must not be able to decide
        // where a run writes, and the two must not be able to disagree.
        Ok(cfg) => Ok(SubagentExtensionConfig {
            roots: Roots::from_config_dirs(dirs),
            ..cfg
        }),
        Err(err) => match raw
            .as_ref()
            .map(SubagentExtensionConfig::invalid_config_disposition)
        {
            Some(InvalidConfigDisposition::Refuse(keys)) => Err(RefusedSubagentConfig {
                path,
                message: err.to_string(),
                keys,
            }),
            _ => warn_and_default(
                &path,
                &format!("is not valid subagents config JSON ({err})"),
                rooted,
            ),
        },
    }
}

/// The non-policy half of SUBA-166: report the bad file on stderr and use the defaults, which is
/// pi's `console.error(...)` + `return {}` (`extension/config.ts:238`).
fn warn_and_default(
    path: &Path,
    what: &str,
    rooted: impl Fn() -> SubagentExtensionConfig,
) -> Result<SubagentExtensionConfig, RefusedSubagentConfig> {
    eprintln!("cyrup: warning: {} {what}; using defaults", path.display());
    Ok(rooted())
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

    /// The layout the binary would have resolved, rooted at a `TempDir`.
    fn dirs_at(dir: &std::path::Path) -> ConfigDirs {
        ConfigDirs {
            agent_dir: dir.to_path_buf(),
            session_dir: dir.join("sessions"),
            session_dir_explicit: false,
            package_dir: dir.join("packages"),
            cwd: dir.to_path_buf(),
            home: dir.to_path_buf(),
        }
    }

    /// What every fall-back path in the loader must produce: the hardcoded defaults, carrying the
    /// roots the BINARY resolved rather than `Default`'s process-derived ones. Asserting against
    /// this (rather than a bare `SubagentExtensionConfig::default()`) is what proves the roots are
    /// set on the absent-file, unparseable and missions-invalid paths too, not just the happy one.
    fn defaults_for(dirs: &ConfigDirs) -> SubagentExtensionConfig {
        SubagentExtensionConfig {
            roots: Roots::from_config_dirs(dirs),
            ..Default::default()
        }
    }

    #[test]
    fn absent_config_json_yields_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dirs = dirs_at(dir.path());
        let cfg = load_subagent_extension_config(&dirs).expect("absent is not an error");
        assert_eq!(cfg, defaults_for(&dirs));
    }

    #[test]
    fn malformed_config_json_falls_back_to_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(subagents_dir.join("config.json"), "not json at all").expect("write");
        let dirs = dirs_at(dir.path());
        let cfg =
            load_subagent_extension_config(&dirs).expect("a file that is not JSON declares no key");
        assert_eq!(cfg, defaults_for(&dirs));
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
        let dirs = dirs_at(dir.path());
        assert_eq!(
            load_subagent_extension_config(&dirs).expect("no fail-closed key is declared"),
            defaults_for(&dirs)
        );
    }

    /// Before this, the loader called only `validate_missions`, so an unknown `authorityPolicy`
    /// action was dropped by serde and the file loaded as if it were fine.
    ///
    /// Mutation killed: calling `validate_missions` instead of `validate_raw_config` — the
    /// file then loads with `maxSubagentDepth: 5`, not the defaults.
    #[test]
    fn an_unknown_authority_action_is_refused_by_name() {
        let raw = serde_json::json!({"authorityPolicy": {"stopRuns": "allow"}});
        let message = SubagentExtensionConfig::validate_raw_config(&raw)
            .expect_err("an unknown authority action must be refused");
        assert!(message.contains("stopRuns"), "names the key: {message}");

        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"maxSubagentDepth": 5, "authorityPolicy": {"stopRuns": "allow"}}"#,
        )
        .expect("write");
        let dirs = dirs_at(dir.path());
        // SUBA-166: `authorityPolicy` is a fail-closed key, so the file is REFUSED rather than
        // replaced by the defaults. Until SUBA-166 this assertion read
        // `assert_eq!(load_subagent_extension_config(&dirs), defaults_for(&dirs))` — i.e. the
        // typo'd action silently lifted the whole policy and the session ran anyway.
        let refused = load_subagent_extension_config(&dirs)
            .expect_err("a file declaring authorityPolicy must not be replaced by the defaults");
        assert_eq!(refused.keys(), ["authorityPolicy"]);
        let shown = refused.to_string();
        assert!(shown.contains("stopRuns"), "names the bad value: {shown}");
        assert!(
            shown.contains(
                &dirs
                    .agent_dir
                    .join("subagents")
                    .join("config.json")
                    .display()
                    .to_string()
            ),
            "names the path: {shown}"
        );
    }

    /// The two other formerly-uncalled validators are reached through the same entry point, and
    /// the refusal carries upstream's own field-naming sentence (`extension/config.ts:152,79`
    /// @v0.68.0) rather than serde's generic "unknown variant"/"invalid value" text — which is all
    /// the typed parse alone would have said, naming no key.
    ///
    /// Mutation killed: dropping either call from `validate_raw_config`.
    #[test]
    fn artifact_dir_and_artifact_config_are_validated_at_load() {
        assert_eq!(
            SubagentExtensionConfig::validate_raw_config(
                &serde_json::json!({"artifactDir": "nowhere"})
            ),
            Err(r#"config.artifactDir must be "project", "session", or "temp""#.to_string())
        );
        assert_eq!(
            SubagentExtensionConfig::validate_raw_config(
                &serde_json::json!({"artifactConfig": {"cleanupDays": -1}})
            ),
            Err("config.artifactConfig.cleanupDays must be a non-negative integer".to_string())
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"maxSubagentDepth": 5, "artifactDir": "nowhere"}"#,
        )
        .expect("write");
        let dirs = dirs_at(dir.path());
        // SUBA-166: no fail-closed key is declared, so warn-and-default is still the answer.
        assert_eq!(
            load_subagent_extension_config(&dirs).expect("no fail-closed key is declared"),
            defaults_for(&dirs)
        );
    }

    /// An upstream key this port never implemented, and a plain typo, are each reported — and
    /// neither takes the rest of the file down.
    ///
    /// Mutation killed: an empty `UNPORTED_CONFIG_KEYS` (the unported key is then reported as
    /// merely unknown), or dropping the unknown-key arm.
    #[test]
    fn an_unported_upstream_key_warns() {
        let raw = serde_json::json!({"maxSubagentDepth": 5, "worktreeProvider": "native", "fleetVeiw": false});
        let warnings = SubagentExtensionConfig::config_warnings(&raw);
        assert_eq!(
            warnings,
            vec![
                "'worktreeProvider' is not supported by this port (worktree allocator selection); it has no effect".to_string(),
                "unknown key 'fleetVeiw' (ignored)".to_string(),
            ]
        );
        // Every key the struct reads is known — including the raw-held ones.
        // CFG-067: `toolTimeoutMs` is among them now. It used to be on `UNPORTED_CONFIG_KEYS`
        // ("a config-level per-tool-call timeout"), i.e. the operator was TOLD it had no effect;
        // the key is read for real now, so warning about it would be a lie. Killing mutation:
        // leaving the entry on the unported list — this assertion then reports it again.
        let quiet = serde_json::json!({"asyncWidget": false, "inlineToolDisplay": "summary", "fleetKeybindings": {}, "completionBatch": {}, "missions": {}, "toolTimeoutMs": 5000});
        assert!(SubagentExtensionConfig::config_warnings(&quiet).is_empty());

        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(subagents_dir.join("config.json"), raw.to_string()).expect("write");
        assert_eq!(
            load_subagent_extension_config(&dirs_at(dir.path()))
                .expect("the file is valid; the keys only warn")
                .max_subagent_depth,
            5
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
        let cfg = load_subagent_extension_config(&dirs_at(dir.path())).expect("a valid file");
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
        let cfg = load_subagent_extension_config(&dirs_at(dir.path())).expect("a valid file");
        assert_eq!(cfg.max_subagent_depth, 5);
        assert_eq!(
            cfg.global_concurrency_limit,
            SubagentExtensionConfig::default().global_concurrency_limit
        );
    }

    /// SUBA-152 — `disabledFeatures` reaching this loader, both ways round.
    ///
    /// A VALID list loads silently: the key is genuine config now, so the
    /// `unknown key 'disabledFeatures' (ignored)` line this loader used to print on every such
    /// file is gone, and the rest of the file is honored alongside it.
    ///
    /// An INVALID one REFUSES the whole file rather than warning, because `disabledFeatures` is
    /// one of [`cyrup_ext_subagents::registration::FAIL_CLOSED_CONFIG_KEYS`] — it was listed there
    /// deliberately ahead of the port, and now that `validate_raw_config` actually checks the key
    /// that listing has teeth: an operator who trimmed the tool must not silently get the FULL
    /// tool back because they typo'd the value. Upstream rethrows the same file
    /// (`extension/config.ts:226-240` @v0.75.0).
    ///
    /// Mutation killed: dropping `validate_disabled_features` from `validate_raw_config` (the bad
    /// file then loads with the operator's trim silently discarded); renaming the config field
    /// (the valid file warns again).
    #[test]
    fn a_disabled_features_list_loads_silently_and_a_bad_one_refuses_the_file() {
        let write = |body: &str| {
            let dir = tempfile::tempdir().expect("tempdir");
            let subagents_dir = dir.path().join("subagents");
            std::fs::create_dir_all(&subagents_dir).expect("mkdir");
            std::fs::write(subagents_dir.join("config.json"), body).expect("write");
            dir
        };

        let dir = write(r#"{"maxSubagentDepth": 5, "disabledFeatures": ["panes", "watchdog"]}"#);
        let raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("subagents").join("config.json")).expect("read"),
        )
        .expect("valid JSON");
        assert!(
            SubagentExtensionConfig::config_warnings(&raw).is_empty(),
            "a ported key must not warn: {:?}",
            SubagentExtensionConfig::config_warnings(&raw)
        );
        let cfg = load_subagent_extension_config(&dirs_at(dir.path())).expect("a valid file");
        assert_eq!(
            cfg.max_subagent_depth, 5,
            "the rest of the file still loads"
        );
        assert_eq!(
            cfg.disabled_features.as_deref(),
            Some(
                &[
                    cyrup_ext_subagents::disabled_features::SubagentFeature::Panes,
                    cyrup_ext_subagents::disabled_features::SubagentFeature::Watchdog,
                ][..]
            )
        );

        // A name that is not one of the fifteen, beside an unrelated setting.
        let dir = write(r#"{"maxSubagentDepth": 5, "disabledFeatures": ["panez"]}"#);
        let refused = load_subagent_extension_config(&dirs_at(dir.path()))
            .expect_err("a declared disabledFeatures must not be replaced by the defaults");
        assert_eq!(refused.keys(), ["disabledFeatures"]);
        let shown = refused.to_string();
        assert!(
            shown.contains(r#"config.disabledFeatures entry "panez" is not one of: "#),
            "carries upstream's own sentence, naming the bad entry: {shown}"
        );
        assert!(
            shown.contains("agent-management, watchdog, panes"),
            "and the full feature list: {shown}"
        );
    }

    /// SUBA-166 — the exact failure pi's `9f1c2552` (#2624) changelog names: an invalid value for
    /// ANY key used to silently drop `authorityPolicy`, `permissions` and `toolBudget`. A typo in
    /// `artifactDir` sits beside a real `authorityPolicy`, and the whole file — restriction
    /// included — was replaced by the all-permissive defaults with one stderr line to show for it.
    ///
    /// Mutation killed: making the `validate_raw_config` failure arm warn-and-default again. The
    /// load then succeeds with `authority_policy: None`, i.e. `stopRun` is no longer forbidden.
    #[test]
    fn a_typo_beside_an_authority_policy_refuses_the_file_instead_of_lifting_the_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"artifactDir": "nowhere", "authorityPolicy": {"stopRun": "forbid"}}"#,
        )
        .expect("write");
        let dirs = dirs_at(dir.path());

        let refused = load_subagent_extension_config(&dirs)
            .expect_err("a declared authorityPolicy must not be replaced by the defaults");

        assert_eq!(refused.keys(), ["authorityPolicy"]);
        let shown = refused.to_string();
        assert!(
            shown.contains(&subagents_dir.join("config.json").display().to_string()),
            "names the path: {shown}"
        );
        assert!(
            shown.contains(r#"config.artifactDir must be "project", "session", or "temp""#),
            "carries the validator's own message, as upstream's rethrow does: {shown}"
        );
        assert!(
            shown.contains("must not be silently discarded"),
            "says why the file was refused: {shown}"
        );
    }

    /// SUBA-166 — the other two keys `9f1c2552` added, one of which (`toolBudget`) this port does
    /// not even read: an unported policy key still means the operator was declaring a policy, so
    /// it fails closed too rather than waiting for the key to be ported.
    ///
    /// Mutation killed: dropping `permissions` or `toolBudget` from `FAIL_CLOSED_CONFIG_KEYS`.
    #[test]
    fn permissions_and_an_unported_tool_budget_fail_closed_too() {
        for (key, body) in [
            ("permissions", r#""permissions": {"mode": "allow"}"#),
            ("toolBudget", r#""toolBudget": {"hard": 5}"#),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let subagents_dir = dir.path().join("subagents");
            std::fs::create_dir_all(&subagents_dir).expect("mkdir");
            std::fs::write(
                subagents_dir.join("config.json"),
                format!(r#"{{"artifactDir": "nowhere", {body}}}"#),
            )
            .expect("write");

            let Err(refused) = load_subagent_extension_config(&dirs_at(dir.path())) else {
                panic!("a file declaring {key} must not be replaced by the defaults");
            };
            assert_eq!(refused.keys(), [key]);
        }
    }

    /// SUBA-178 — `runnerLaunchers` is read only from this user file, validated by upstream's
    /// `validateRunnerLaunchersConfig` (`extension/config.ts:106-116` @ad11b7ab) and fail-closed
    /// (`:17`): an invalid value refuses the WHOLE file, so a `maxSubagentDepth` beside it is not
    /// applied either. A valid map loads typed and without the "unknown key" warning.
    ///
    /// Mutations killed: dropping `runnerLaunchers` from `FAIL_CLOSED_CONFIG_KEYS` (the load
    /// returns the defaults); dropping the validator call (`[]` parses typed and loads); dropping
    /// the struct field (the valid case yields no launchers). Base tree: the bad file loaded with a
    /// warning.
    #[test]
    fn an_invalid_runner_launchers_value_refuses_the_whole_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"runnerLaunchers": {"net": []}, "maxSubagentDepth": 3}"#,
        )
        .expect("write");
        let Err(refused) = load_subagent_extension_config(&dirs_at(dir.path())) else {
            panic!("an invalid runnerLaunchers value must refuse the whole file");
        };
        assert_eq!(refused.keys(), ["runnerLaunchers"]);
        assert!(
            refused.to_string().contains(
                r#"config.runnerLaunchers["net"] must be a non-empty argv array of non-blank strings without NUL characters"#
            ),
            "{refused}"
        );

        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"runnerLaunchers": {"net": ["env", "--", "X=1"]}, "maxSubagentDepth": 3}"#,
        )
        .expect("write");
        let cfg = load_subagent_extension_config(&dirs_at(dir.path())).expect("valid map loads");
        assert_eq!(cfg.max_subagent_depth, 3);
        let launchers = cfg.runner_launchers.expect("runnerLaunchers is typed");
        assert_eq!(
            launchers.get("net").map(Vec::as_slice),
            Some(["env".to_owned(), "--".to_owned(), "X=1".to_owned()].as_slice())
        );
        assert!(
            SubagentExtensionConfig::config_warnings(&serde_json::json!({
                "runnerLaunchers": {"net": ["env"]}
            }))
            .is_empty(),
            "a typed key must not warn as unknown"
        );
    }

    /// SUBA-166 — the typed-parse arm fails closed on the same list. `maxSubagentDepth: "five"`
    /// passes every RAW validator (none of them looks at it) and dies in serde, which is the other
    /// `return rooted()` the row names (`subagent_config.rs:91-97` before this change).
    ///
    /// Mutation killed: leaving the typed-parse arm on `warn_and_default`. The load then succeeds
    /// with `scheduled_runs: None`, i.e. the operator's scheduled-runs policy is gone.
    #[test]
    fn a_typed_parse_failure_beside_a_policy_key_refuses_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"maxSubagentDepth": "five", "scheduledRuns": {"enabled": true}}"#,
        )
        .expect("write");

        let refused = load_subagent_extension_config(&dirs_at(dir.path()))
            .expect_err("a declared scheduledRuns policy must not be replaced by the defaults");

        assert_eq!(refused.keys(), ["scheduledRuns"]);
    }

    /// SUBA-166's other half, and the reason the list exists at all: a bad-but-present file that
    /// declares NO policy key keeps this loader's warn-and-default convention, which is upstream's
    /// `console.error` + `return {}` (`extension/config.ts:238`).
    ///
    /// Mutation killed: failing closed unconditionally — every malformed optional file would then
    /// abort startup, which is neither this loader's convention nor pi's.
    #[test]
    fn a_bad_file_that_declares_no_policy_key_still_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let subagents_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&subagents_dir).expect("mkdir");
        std::fs::write(
            subagents_dir.join("config.json"),
            r#"{"artifactDir": "nowhere", "maxSubagentDepth": 5}"#,
        )
        .expect("write");
        let dirs = dirs_at(dir.path());

        assert_eq!(
            load_subagent_extension_config(&dirs).expect("no policy key is declared"),
            defaults_for(&dirs)
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

        let cfg = load_subagent_extension_config(&dirs_at(dir.path())).expect("a valid file");

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
