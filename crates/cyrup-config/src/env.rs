//! Directory + environment resolution — the single env touchpoint (arch-07 §3.1, R-07-003/028).

use std::path::PathBuf;

use crate::error::ConfigError;

/// Cache-retention policy honoured from `CYRUP_CACHE_RETENTION` (← `PI_CACHE_RETENTION`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheRetention {
    #[default]
    Short,
    Long,
}

impl CacheRetention {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "short" => Some(Self::Short),
            "long" => Some(Self::Long),
            _ => None,
        }
    }
}

/// Typed view over the `CYRUP_*` environment surface, with `PI_*` accepted as a migration
/// fallback (documented; R-07-028). This is the ONLY place process env is read.
#[derive(Clone, Debug, Default)]
pub struct EnvVars {
    pub agent_dir: Option<PathBuf>,
    pub session_dir: Option<PathBuf>,
    pub package_dir: Option<PathBuf>,
    pub offline: bool,
    pub skip_version_chk: bool,
    /// `None` = unset (telemetry policy then defers to settings).
    pub telemetry: Option<bool>,
    pub cache_retention: CacheRetention,
    pub visual: Option<String>,
    pub editor: Option<String>,
    pub http_proxy: Option<String>,
    /// `CYRUP_CLEAR_ON_SHRINK` (← `PI_CLEAR_ON_SHRINK`) — true only when the value is exactly `"1"`
    /// (Pi `getClearOnShrink`, settings-manager.ts:1082). Used as the env fallback when the
    /// `terminal.clearOnShrink` setting is absent.
    pub clear_on_shrink: bool,
    /// `CYRUP_HARDWARE_CURSOR` (← `PI_HARDWARE_CURSOR`) — true only when the value is exactly `"1"`
    /// (Pi `getShowHardwareCursor`, settings-manager.ts:1166). Env fallback for the
    /// `showHardwareCursor` setting.
    pub hardware_cursor: bool,
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
}

/// Port of Pi `isTruthyEnvFlag` (telemetry.ts:3-6, main.ts:95-98, package-manager.ts:42-46):
/// a flag env is truthy only when it is exactly `1`, or case-insensitively `true`/`yes`. Pi does
/// NOT trim or accept `on`, so `CYRUP_TELEMETRY=on` / `PI_TELEMETRY=on` must NOT enable telemetry
/// (and likewise for `*_OFFLINE` / `*_SKIP_VERSION_CHECK`, which use the same flag predicate).
fn truthy(s: &str) -> bool {
    s == "1" || s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("yes")
}

impl EnvVars {
    /// Reads the process environment once.
    pub fn from_process() -> Self {
        let path = |keys: &[&str]| first_env(keys).map(PathBuf::from);
        Self {
            agent_dir: path(&["CYRUP_AGENT_DIR", "PI_CODING_AGENT_DIR"]),
            session_dir: path(&["CYRUP_SESSION_DIR", "PI_CODING_AGENT_SESSION_DIR"]),
            package_dir: path(&["CYRUP_PACKAGE_DIR", "PI_PACKAGE_DIR"]),
            offline: first_env(&["CYRUP_OFFLINE", "PI_OFFLINE"])
                .as_deref()
                .is_some_and(truthy),
            skip_version_chk: first_env(&["CYRUP_SKIP_VERSION_CHECK", "PI_SKIP_VERSION_CHECK"])
                .as_deref()
                .is_some_and(truthy),
            telemetry: first_env(&["CYRUP_TELEMETRY", "PI_TELEMETRY"])
                .as_deref()
                .map(truthy),
            cache_retention: first_env(&["CYRUP_CACHE_RETENTION", "PI_CACHE_RETENTION"])
                .as_deref()
                .and_then(CacheRetention::parse)
                .unwrap_or_default(),
            visual: first_env(&["VISUAL"]),
            editor: first_env(&["EDITOR"]),
            http_proxy: first_env(&["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"]),
            // Pi compares strictly to "1" (settings-manager.ts:1082,1166), not the broader truthy set.
            clear_on_shrink: first_env(&["CYRUP_CLEAR_ON_SHRINK", "PI_CLEAR_ON_SHRINK"]).as_deref()
                == Some("1"),
            hardware_cursor: first_env(&["CYRUP_HARDWARE_CURSOR", "PI_HARDWARE_CURSOR"]).as_deref()
                == Some("1"),
        }
    }
}

/// Typed CLI overrides consumed by the config layer (arch-11 parses; we accept the typed form).
#[derive(Clone, Debug, Default)]
pub struct CliConfigOverrides {
    pub agent_dir: Option<PathBuf>,
    pub session_dir: Option<PathBuf>,
    pub package_dir: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    /// `--offline`.
    pub offline: bool,
    /// `--approve` (Some(true)) / `--no-approve` (Some(false)); None = no override.
    pub trust_override: Option<bool>,
    /// `--model` / `--models` patterns for this run.
    pub model: Option<String>,
    pub models: Vec<String>,
    /// `--api-key` per-run override.
    pub api_key: Option<String>,
}

/// Resolved directory knobs (arch-07 §3.1). Precedence per knob: CLI > env > default.
#[derive(Clone, Debug)]
pub struct ConfigDirs {
    pub agent_dir: PathBuf,
    pub session_dir: PathBuf,
    /// Whether `session_dir` came from an explicit `--session-dir` flag or `$CYRUP_SESSION_DIR`
    /// (as opposed to the `agent_dir/sessions` default). Pi keeps this distinction as the optional
    /// `sessionDir?` argument threaded through every session op; `ConfigDirs::resolve` otherwise
    /// collapses it, so it is preserved here for the layout to use the explicit dir LITERALLY vs.
    /// cwd-encoding the default (gap-analysis 05, Finding 3; Pi `sessionDir ? … : getDefaultSessionDir`).
    pub session_dir_explicit: bool,
    pub package_dir: PathBuf,
    pub cwd: PathBuf,
    /// The real user home directory (`process.env.HOME || homedir()`; Pi `getHomeDir()`,
    /// package-manager.ts:217, and trust-manager.ts:185). This is the SAME home Pi uses to anchor
    /// `~/.agents/skills` (the user-tier cross-tool skills dir excluded from the project
    /// `.agents/skills` ancestor walk) and the trust-requiring-resource walk — NOT the agent dir.
    /// Threaded onto `SessionConfig.home` so discovery + trust detection resolve against the real
    /// home, matching Pi. (`resolve` errors early when the home cannot be determined at all.)
    pub home: PathBuf,
}

impl ConfigDirs {
    /// Resolve the directory layout. A missing home falls back to the current dir rather than
    /// panicking (R-00-009).
    pub fn resolve(cli: &CliConfigOverrides, env: &EnvVars) -> Result<Self, ConfigError> {
        let home = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .ok_or_else(|| ConfigError::Dir("could not determine home directory".to_string()))?;

        let agent_dir = cli
            .agent_dir
            .clone()
            .or_else(|| env.agent_dir.clone())
            .unwrap_or_else(|| home.join(".cyrup").join("agent"));

        // Tiers 1 and 2 of Pi's three-tier `sessionDir` chain (main.ts:625-630). The third —
        // `startupSettingsManager.getSessionDir()` — is applied afterwards by the bin via
        // [`ConfigDirs::with_settings_session_dir`]; see that method for why it cannot live here.
        let session_dir_override = cli.session_dir.clone().or_else(|| env.session_dir.clone());
        let session_dir_explicit = session_dir_override.is_some();
        let session_dir =
            session_dir_override.unwrap_or_else(|| agent_dir.join("sessions"));

        let package_dir = cli
            .package_dir
            .clone()
            .or_else(|| env.package_dir.clone())
            .unwrap_or_else(|| agent_dir.join("packages"));

        let cwd = match cli.cwd.clone() {
            Some(p) => p,
            None => std::env::current_dir().map_err(ConfigError::Io)?,
        };
        // Canonicalize when possible; otherwise keep the literal path (R-07-013 keys are canonical
        // but a not-yet-existing cwd must not crash startup).
        let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);

        Ok(Self {
            agent_dir,
            session_dir,
            session_dir_explicit,
            package_dir,
            cwd,
            home,
        })
    }

    /// Apply the third and lowest-precedence `sessionDir` tier — the merged `settings.json` key —
    /// on top of an already-resolved layout. Pi resolves the whole chain in one expression
    /// (main.ts:625-630):
    ///
    /// ```text
    /// const sessionDir =
    ///     (parsed.sessionDir ? normalizePath(parsed.sessionDir) : undefined) ??
    ///     (envSessionDir ? expandTildePath(envSessionDir) : undefined) ??
    ///     startupSettingsManager.getSessionDir();
    /// ```
    ///
    /// where `getSessionDir()` returns the merged global+project `settings.sessionDir`
    /// (settings-manager.ts:670-673). Tiers 1 and 2 are folded in by [`ConfigDirs::resolve`]; this
    /// method is the tier-3 fallback and it deliberately takes the value as an argument instead of
    /// reading a file: the settings live under the `agent_dir`/`cwd` that `resolve` is what
    /// computes, so the caller must resolve the layout first. Pi has the identical ordering — its
    /// `startupSettingsManager` is constructed *after* the dirs, at main.ts:610 — so the settings
    /// I/O stays in the bin and `cyrup-config` never touches the filesystem to resolve directories.
    ///
    /// Precedence and edge cases, matching Pi's `??` chain:
    /// - a `--session-dir`/`$CYRUP_SESSION_DIR` override wins outright (the flag is already
    ///   `session_dir_explicit`, so this is a no-op);
    /// - an absent or blank value leaves the `<agent_dir>/sessions` default in place. Pi threads a
    ///   `""` through `??` unchanged, but every consumer re-tests it as
    ///   `sessionDir ? normalizePath(sessionDir) : getDefaultSessionDir(cwd)`
    ///   (session-manager.ts:1538), so a blank setting resolves to the default either way.
    ///
    /// When the tier fires, `session_dir_explicit` becomes **true**: Pi passes a settings-derived
    /// `sessionDir` into `createSessionManager(parsed, cwd, sessionDir, …)` (main.ts:630) through
    /// the very same argument slot as `--session-dir`, so it is used LITERALLY rather than
    /// cwd-encoded (see the field docs on [`ConfigDirs::session_dir_explicit`]).
    #[must_use]
    pub fn with_settings_session_dir(mut self, settings_session_dir: Option<PathBuf>) -> Self {
        if self.session_dir_explicit {
            return self;
        }
        let Some(dir) = settings_session_dir else {
            return self;
        };
        if dir.to_string_lossy().trim().is_empty() {
            return self;
        }
        self.session_dir = dir;
        self.session_dir_explicit = true;
        self
    }

    pub fn settings_path(&self) -> PathBuf {
        self.agent_dir.join("settings.json")
    }
    pub fn trust_path(&self) -> PathBuf {
        self.agent_dir.join("trust.json")
    }
    pub fn auth_path(&self) -> PathBuf {
        self.agent_dir.join("auth.json")
    }
    pub fn project_config_dir(&self) -> PathBuf {
        self.cwd.join(".cyrup")
    }
    pub fn project_settings_path(&self) -> PathBuf {
        self.project_config_dir().join("settings.json")
    }

    // Additional global config-dir paths (Pi `ConfigDirs`, config.ts:524-566). All sit under the
    // agent dir alongside settings/trust/auth.
    /// `models.json` — custom-model / provider-config file (consumed by `load_models_file`).
    pub fn models_path(&self) -> PathBuf {
        self.agent_dir.join("models.json")
    }
    pub fn themes_dir(&self) -> PathBuf {
        self.agent_dir.join("themes")
    }
    pub fn tools_dir(&self) -> PathBuf {
        self.agent_dir.join("tools")
    }
    pub fn bin_dir(&self) -> PathBuf {
        self.agent_dir.join("bin")
    }
    pub fn prompts_dir(&self) -> PathBuf {
        self.agent_dir.join("prompts")
    }
    pub fn debug_log_path(&self) -> PathBuf {
        self.agent_dir.join("debug.log")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{CliConfigOverrides, ConfigDirs, EnvVars};
    use std::path::PathBuf;

    #[test]
    fn resolve_captures_real_home_distinct_from_agent_dir() {
        // G1: `ConfigDirs::resolve` must retain the real user home (Pi `getHomeDir()`,
        // package-manager.ts:217; `getAgentDir` = `join(homedir(), CONFIG_DIR_NAME, "agent")`,
        // config.ts:520) rather than discarding it. With no env/CLI agent-dir override the agent dir
        // defaults to `<home>/.cyrup/agent`, so `home` is a strict ancestor and must differ.
        let env = EnvVars::default();
        let cli = CliConfigOverrides {
            cwd: Some(PathBuf::from("/")),
            ..Default::default()
        };
        let dirs = ConfigDirs::resolve(&cli, &env).unwrap();
        assert_eq!(dirs.agent_dir, dirs.home.join(".cyrup").join("agent"));
        assert_ne!(dirs.home, dirs.agent_dir);
        assert!(dirs.agent_dir.starts_with(&dirs.home));
    }

    /// Tier 3 of Pi's `sessionDir` chain (main.ts:625-630): with neither `--session-dir` nor
    /// `$CYRUP_SESSION_DIR`, the merged `settings.json` key wins over the `<agent_dir>/sessions`
    /// default — and counts as EXPLICIT, because Pi hands it to `createSessionManager` in the same
    /// argument slot as the flag (main.ts:630), which selects the literal (not cwd-encoded) layout.
    #[test]
    fn settings_session_dir_overrides_the_default_and_is_explicit() {
        let env = EnvVars::default();
        let cli = CliConfigOverrides {
            cwd: Some(PathBuf::from("/")),
            ..Default::default()
        };
        let dirs = ConfigDirs::resolve(&cli, &env).unwrap();
        assert_eq!(dirs.session_dir, dirs.agent_dir.join("sessions"));
        assert!(!dirs.session_dir_explicit);

        let dirs = dirs.with_settings_session_dir(Some(PathBuf::from("/work/sessions")));
        assert_eq!(dirs.session_dir, PathBuf::from("/work/sessions"));
        assert!(dirs.session_dir_explicit);
    }

    /// `??` short-circuits: an explicit `--session-dir`/`$CYRUP_SESSION_DIR` is never reached by the
    /// settings tier (Pi main.ts:625-627). Blank and absent settings leave the default standing —
    /// Pi threads `""` through unchanged but every consumer re-tests it with
    /// `sessionDir ? … : getDefaultSessionDir(cwd)` (session-manager.ts:1538).
    #[test]
    fn settings_session_dir_yields_to_cli_env_and_ignores_blanks() {
        let env = EnvVars::default();
        let cli = CliConfigOverrides {
            cwd: Some(PathBuf::from("/")),
            session_dir: Some(PathBuf::from("/flag/sessions")),
            ..Default::default()
        };
        let dirs = ConfigDirs::resolve(&cli, &env)
            .unwrap()
            .with_settings_session_dir(Some(PathBuf::from("/settings/sessions")));
        assert_eq!(dirs.session_dir, PathBuf::from("/flag/sessions"));

        let cli = CliConfigOverrides {
            cwd: Some(PathBuf::from("/")),
            ..Default::default()
        };
        let base = ConfigDirs::resolve(&cli, &env).unwrap();
        let default_dir = base.agent_dir.join("sessions");
        for absent in [None, Some(PathBuf::from("")), Some(PathBuf::from("  "))] {
            let dirs = base.clone().with_settings_session_dir(absent);
            assert_eq!(dirs.session_dir, default_dir);
            assert!(!dirs.session_dir_explicit);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod flag_tests {
    use super::truthy;

    #[test]
    fn truthy_matches_pi_is_truthy_env_flag() {
        // Pi isTruthyEnvFlag (telemetry.ts:3-6): only 1 / true / yes (case-insensitive for the
        // latter two); `on` is NOT accepted, and there is no trimming.
        assert!(truthy("1"));
        assert!(truthy("true"));
        assert!(truthy("TRUE"));
        assert!(truthy("yes"));
        assert!(truthy("Yes"));
        // Divergence that was the gap: `on` must be falsy.
        assert!(!truthy("on"));
        assert!(!truthy("ON"));
        assert!(!truthy("0"));
        assert!(!truthy("false"));
        assert!(!truthy(""));
        // Pi does not trim, so surrounding whitespace is not truthy.
        assert!(!truthy(" 1"));
        assert!(!truthy("1 "));
    }
}
