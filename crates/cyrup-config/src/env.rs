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
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
}

fn truthy(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

impl EnvVars {
    /// Reads the process environment once.
    pub fn from_process() -> Self {
        let path = |keys: &[&str]| first_env(keys).map(PathBuf::from);
        Self {
            agent_dir: path(&["CYRUP_AGENT_DIR", "PI_CODING_AGENT_DIR"]),
            session_dir: path(&["CYRUP_SESSION_DIR", "PI_CODING_AGENT_SESSION_DIR"]),
            package_dir: path(&["CYRUP_PACKAGE_DIR", "PI_PACKAGE_DIR"]),
            offline: first_env(&["CYRUP_OFFLINE", "PI_OFFLINE"]).as_deref().is_some_and(truthy),
            skip_version_chk: first_env(&["CYRUP_SKIP_VERSION_CHECK", "PI_SKIP_VERSION_CHECK"])
                .as_deref()
                .is_some_and(truthy),
            telemetry: first_env(&["CYRUP_TELEMETRY", "PI_TELEMETRY"]).as_deref().map(truthy),
            cache_retention: first_env(&["CYRUP_CACHE_RETENTION", "PI_CACHE_RETENTION"])
                .as_deref()
                .and_then(CacheRetention::parse)
                .unwrap_or_default(),
            visual: first_env(&["VISUAL"]),
            editor: first_env(&["EDITOR"]),
            http_proxy: first_env(&["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"]),
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
    pub package_dir: PathBuf,
    pub cwd: PathBuf,
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

        let session_dir = cli
            .session_dir
            .clone()
            .or_else(|| env.session_dir.clone())
            .unwrap_or_else(|| agent_dir.join("sessions"));

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

        Ok(Self { agent_dir, session_dir, package_dir, cwd })
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
}
