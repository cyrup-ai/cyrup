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
    /// The RESOLVED home directory — [`crate::paths::cyrup_home_dir_from`]'s full ladder
    /// (`CYRUP_HOME` -> `HOME` -> OS home), run once here at the crate's single env touchpoint.
    ///
    /// This rung was absent from this type until now, and its absence is the whole of the
    /// divergence it closes: five other crates hand-rolled a `CYRUP_HOME` ladder precisely because
    /// the type that owns layout resolution did not carry one. `None` when nothing answers, which
    /// [`ConfigDirs::resolve`] turns into an error rather than a silent `/tmp`.
    pub home: Option<PathBuf>,
    /// The ENVIRONMENT tier of the agent-dir override, already `~`-expanded against [`Self::home`]
    /// ([`crate::paths::cyrup_agent_dir_from`]). `None` when no key is set, so the CLI tier and the
    /// `<home>/.cyrup/agent` default still layer above and below it in [`ConfigDirs::resolve`].
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
        Self::from_lookup(|key| std::env::var_os(key))
    }

    /// The whole env mapping, driven by an arbitrary key lookup.
    ///
    /// `from_process` passes `std::env::var`. A test passes a fixed table: `std::env::set_var` is
    /// `unsafe` under Rust 2024 and this crate is `#![forbid(unsafe_code)]`, so the process
    /// environment is not writable from here at all — and would race sibling tests in the same
    /// binary if it were. `get` returning `Some("")` is a **set but empty** variable, which is a
    /// meaningful third state for `telemetry` (DRIFT-050).
    pub fn from_lookup(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Self {
        // The one lookup, in the one shape the shared ladders take. `OsString` because these
        // answers become paths and a `String` seam drops a non-UTF-8 value to the next rung; the
        // non-path fields below convert with `to_string_lossy`, which is what they already got.
        let lookup: crate::paths::EnvLookup<'_> = &|key: &str| get(key);
        let text = |key: &str| get(key).map(|v| v.to_string_lossy().into_owned());
        // Pi's `first_env` shape: first key that is set to a NON-EMPTY value.
        let first = |keys: &[&str]| keys.iter().find_map(|k| text(k).filter(|v| !v.is_empty()));
        // THE home ladder, run once. Every dir override below expands its `~` against THIS home
        // rather than an ambient one — see `paths::cyrup_dir_override_from` for the divergence
        // that closes.
        let home = crate::paths::cyrup_home_dir_from(lookup);
        // Pi normalizes every dir env var as it reads it — `getAgentDir()` is
        // `if (envDir) { return expandTildePath(envDir); }` (config.ts:515-521 @v0.83.0) and
        // `getPackageDir()` the same for `PI_PACKAGE_DIR` (`:367-372`); the session-dir env tier is
        // `expandTildePath(envSessionDir)` at main.ts:625-628. `expandTildePath` IS `normalizePath`
        // (config.ts:498-500). CFG-036.
        let path =
            |keys: &[&str]| crate::paths::cyrup_dir_override_from(keys, home.as_deref(), lookup);
        // Resolved before the struct literal so the `path` closure's borrow of `home` ends before
        // `home` itself is moved into the field.
        let agent_dir = path(&crate::paths::ENV_AGENT_DIR_KEYS);
        let session_dir = path(&["CYRUP_SESSION_DIR", "PI_CODING_AGENT_SESSION_DIR"]);
        let package_dir = path(&["CYRUP_PACKAGE_DIR", "PI_PACKAGE_DIR"]);
        Self {
            home,
            // CFG-076 — upstream has exactly ONE agent-dir env name, `PI_CODING_AGENT_DIR`, and
            // pi core, `pi-intercom` (`broker/paths.ts:27-38`, asserted at `broker/paths.test.ts:25`)
            // and `pi-subagents` (`src/shared/utils.ts:96`, `src/agents/agents.ts:1886`) all read
            // that same name — so upstream, one variable moves every tree at once. cyrup's rename
            // split it in two: core took the SHORT `CYRUP_AGENT_DIR` (which is what `--help`
            // advertises, `cli.rs:895`) while the two sibling ports took the mechanical long form
            // `CYRUP_CODING_AGENT_DIR` (`cyrup-intercom/src/paths.rs:18`,
            // `cyrup-ext-subagents/src/native_supervisor.rs:1772`). Reading BOTH here restores
            // upstream's one-name-one-tree property from the operator's side: whichever spelling is
            // set, core lands on the same directory the siblings do. The short name stays FIRST so
            // the documented spelling wins when both are set. `PI_CODING_AGENT_DIR` remains last as
            // the migration fallback (R-07-028).
            agent_dir,
            session_dir,
            package_dir,
            offline: first(&["CYRUP_OFFLINE", "PI_OFFLINE"])
                .as_deref()
                .is_some_and(truthy),
            skip_version_chk: first(&["CYRUP_SKIP_VERSION_CHECK", "PI_SKIP_VERSION_CHECK"])
                .as_deref()
                .is_some_and(truthy),
            // TRI-STATE, unlike its two siblings above: unset / set-empty / set-truthy. Pi's
            // `isInstallTelemetryEnabled` branches on `telemetryEnv !== undefined`
            // (telemetry.ts:8-12 @v0.83.0), so `PI_TELEMETRY=` takes the ENV branch and
            // `isTruthyEnvFlag("")` is false at `:3-5` — an explicit OFF that beats the settings
            // value at `policy.rs:25-27`. Filtering the empty string here silently kept telemetry
            // ON for `PI_TELEMETRY= cyrup …`, the ordinary way to neutralise an inherited variable
            // (DRIFT-050).
            telemetry: ["CYRUP_TELEMETRY", "PI_TELEMETRY"]
                .iter()
                .find_map(|k| text(k))
                .as_deref()
                .map(truthy),
            cache_retention: first(&["CYRUP_CACHE_RETENTION", "PI_CACHE_RETENTION"])
                .as_deref()
                .and_then(CacheRetention::parse)
                .unwrap_or_default(),
            visual: first(&["VISUAL"]),
            editor: first(&["EDITOR"]),
            http_proxy: first(&["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"]),
            // Pi compares strictly to "1" (settings-manager.ts:1082,1166), not the broader truthy set.
            clear_on_shrink: first(&["CYRUP_CLEAR_ON_SHRINK", "PI_CLEAR_ON_SHRINK"]).as_deref()
                == Some("1"),
            hardware_cursor: first(&["CYRUP_HARDWARE_CURSOR", "PI_HARDWARE_CURSOR"]).as_deref()
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
        // THE home ladder (`CYRUP_HOME` -> `HOME` -> OS home) was already run once by
        // `EnvVars::from_lookup`, this crate's single env touchpoint. This is the BINARY terminal
        // for it: a `cyrup` that cannot locate home refuses rather than silently laying its layout
        // down in a temp directory. `cyrup_ext_subagents::paths::home_dir` takes the other terminal
        // for the opposite and equally correct reason — a library must not panic.
        let home = env
            .home
            .clone()
            .ok_or_else(|| ConfigError::Dir("could not determine home directory".to_string()))?;

        // Pi normalizes the CLI tier too — `parsed.sessionDir ? normalizePath(parsed.sessionDir)`
        // (main.ts:625-626 @v0.83.0) — so a quoted `--session-dir ~/sessions`, or one supplied from
        // a config file or CI variable where no shell expanded it, resolves under `$HOME` instead
        // of creating a directory literally named `~`. CFG-036.
        //
        // Anchored on the home resolved ABOVE, not on `paths::ambient_home`: with a `CYRUP_HOME`
        // set, a `~/…` flag and a `~/…` env override must land in the same tree as everything else
        // this function derives.
        let normalize_cli = |p: &PathBuf| {
            PathBuf::from(crate::paths::normalize_path_with_home(
                &p.to_string_lossy(),
                Some(&home),
            ))
        };

        let agent_dir = cli
            .agent_dir
            .as_ref()
            .map(normalize_cli)
            .or_else(|| env.agent_dir.clone())
            .unwrap_or_else(|| home.join(".cyrup").join("agent"));

        // Tiers 1 and 2 of Pi's three-tier `sessionDir` chain (main.ts:625-630). The third —
        // `startupSettingsManager.getSessionDir()` — is applied afterwards by the bin via
        // [`ConfigDirs::with_settings_session_dir`]; see that method for why it cannot live here.
        let session_dir_override = cli
            .session_dir
            .as_ref()
            .map(normalize_cli)
            .or_else(|| env.session_dir.clone());
        let session_dir_explicit = session_dir_override.is_some();
        let session_dir = session_dir_override.unwrap_or_else(|| agent_dir.join("sessions"));

        let package_dir = cli
            .package_dir
            .as_ref()
            .map(normalize_cli)
            .or_else(|| env.package_dir.clone())
            .unwrap_or_else(|| agent_dir.join("packages"));

        // SESS-036's residual: this used to end in `std::fs::canonicalize(&cwd).unwrap_or(cwd)`,
        // justified as "R-07-013 keys are canonical". That justification does not hold and the call
        // was a divergence:
        //
        // 1. pi never realpaths the runtime cwd. `main.ts:534` @v0.83.0 is `const cwd =
        //    process.cwd();`, used VERBATIM, and every downstream consumer that wants an absolute
        //    form applies `resolvePath` — node's LEXICAL `path.resolve` (`utils/paths.ts:81-85`) —
        //    never `canonicalizePath` (`paths.ts:26-32`, `realpathSync`), which pi reserves for the
        //    two places that genuinely compare realpaths: `trust-manager.ts:39-40` `normalizeCwd`
        //    and `resource-loader.ts:102-113` `findShadowedContextFile`.
        // 2. R-07-013 is satisfied WITHOUT this call, exactly as it is upstream: `TrustStore::nearest`
        //    canonicalizes the cwd it is handed itself (`trust.rs:147`), mirroring pi's
        //    `normalizeCwd` inside `trust-manager.ts`. Nothing else in the tree needs a realpath here.
        // 3. On unix the call was a no-op that hid a real defect: `std::env::current_dir()` is
        //    `getcwd(3)`, which already returns the PHYSICAL path — the same string node's
        //    `process.cwd()` returns — so canonicalizing it changed nothing and the divergence was
        //    invisible on every developer machine. **On Windows it is not a no-op:**
        //    `std::fs::canonicalize` converts to extended-length (`\\?\`-verbatim) syntax, while
        //    node's `path.resolve` never does. That prefix would have flowed into `encode_cwd`'s
        //    session-directory name, the `"cwd"` field written into every session header, the
        //    `<project_instructions path="…">` attribute shown to the model, and the
        //    `sessionCwdMatches` comparison against headers written by any other tool — a
        //    JS→Rust mechanism gap with no JS counterpart, since JS has no verbatim-path concept.
        let cwd = match cli.cwd.clone() {
            // pi has no `--cwd` flag, so there is no upstream line for this slot; its nearest
            // analogue is the caller-supplied cwd `SessionManager` accepts, normalized with
            // `this.cwd = resolvePath(cwd)` (session-manager.ts:876 @v0.83.0). Same rule here:
            // lexical resolve against the process cwd, no filesystem access, no symlink resolution.
            Some(p) => {
                let base = std::env::current_dir().map_err(|e| {
                    ConfigError::Dir(format!("could not determine current directory: {e}"))
                })?;
                crate::paths::resolve_path_from_base(&p.to_string_lossy(), &base)
            }
            // pi `main.ts:534`: `const cwd = process.cwd();` — verbatim.
            None => std::env::current_dir().map_err(|e| {
                ConfigError::Dir(format!("could not determine current directory: {e}"))
            })?,
        };

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
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// An environment that names its home explicitly.
    ///
    /// `EnvVars::default()` is the empty environment, and in an empty environment there is no home
    /// — [`ConfigDirs::resolve`] now says so with an error rather than quietly consulting the
    /// developer's real one. Every layout test below is about path ARITHMETIC, so it states the
    /// home it wants and gets the same answer on every machine.
    fn env_rooted_at(home: &str) -> EnvVars {
        EnvVars {
            home: Some(PathBuf::from(home)),
            ..Default::default()
        }
    }

    /// The fixed home every layout test in this module resolves against.
    const FIXTURE_HOME: &str = "/fixture-home";

    #[test]
    fn resolve_captures_real_home_distinct_from_agent_dir() {
        // G1: `ConfigDirs::resolve` must retain the real user home (Pi `getHomeDir()`,
        // package-manager.ts:217; `getAgentDir` = `join(homedir(), CONFIG_DIR_NAME, "agent")`,
        // config.ts:520) rather than discarding it. With no env/CLI agent-dir override the agent dir
        // defaults to `<home>/.cyrup/agent`, so `home` is a strict ancestor and must differ.
        let env = env_rooted_at(FIXTURE_HOME);
        let cli = CliConfigOverrides {
            cwd: Some(PathBuf::from("/")),
            ..Default::default()
        };
        let dirs = ConfigDirs::resolve(&cli, &env).unwrap();
        assert_eq!(dirs.home, PathBuf::from(FIXTURE_HOME));
        assert_eq!(dirs.agent_dir, dirs.home.join(".cyrup").join("agent"));
        assert_ne!(dirs.home, dirs.agent_dir);
        assert!(dirs.agent_dir.starts_with(&dirs.home));
    }

    /// CFG-076 — upstream's ONE agent-dir env name is read by pi core AND both sibling repos
    /// (`pi-intercom/broker/paths.test.ts:25` asserts `getAgentDirPath` honors
    /// `PI_CODING_AGENT_DIR`; `pi-subagents/src/shared/utils.ts:96` reads the same name), so
    /// setting it moves every tree at once. cyrup's rename split that name across crates —
    /// `CYRUP_AGENT_DIR` in core, `CYRUP_CODING_AGENT_DIR` in `cyrup-intercom` /
    /// `cyrup-ext-subagents` — so core now reads BOTH and an operator gets one tree either way.
    #[test]
    fn both_spellings_of_the_agent_dir_env_reach_the_core_resolver() {
        let long = EnvVars::from_lookup(|k| {
            (k == "CYRUP_CODING_AGENT_DIR").then(|| OsString::from("/opt/long"))
        });
        assert_eq!(long.agent_dir, Some(PathBuf::from("/opt/long")));

        let short = EnvVars::from_lookup(|k| {
            (k == "CYRUP_AGENT_DIR").then(|| OsString::from("/opt/short"))
        });
        assert_eq!(short.agent_dir, Some(PathBuf::from("/opt/short")));

        // The documented spelling wins when both are set, and the `PI_` migration fallback stays
        // last.
        let both = EnvVars::from_lookup(|k| match k {
            "CYRUP_AGENT_DIR" => Some(OsString::from("/opt/short")),
            "CYRUP_CODING_AGENT_DIR" => Some(OsString::from("/opt/long")),
            "PI_CODING_AGENT_DIR" => Some(OsString::from("/opt/legacy")),
            _ => None,
        });
        assert_eq!(both.agent_dir, Some(PathBuf::from("/opt/short")));

        let legacy = EnvVars::from_lookup(|k| match k {
            "CYRUP_CODING_AGENT_DIR" => Some(OsString::from("/opt/long")),
            "PI_CODING_AGENT_DIR" => Some(OsString::from("/opt/legacy")),
            _ => None,
        });
        assert_eq!(legacy.agent_dir, Some(PathBuf::from("/opt/long")));
    }

    /// CFG-036: Pi normalizes EVERY directory tier, not only the settings one — the CLI flag at
    /// main.ts:625-626 (`normalizePath(parsed.sessionDir)`) and the env vars at config.ts:515-521 /
    /// `:367-372` (`expandTildePath(envDir)`). At HEAD before this fix, `path()` was
    /// `first_env(keys).map(PathBuf::from)` and the CLI overrides were cloned verbatim, so a `~`
    /// survived into the resolved layout as a literal directory component.
    #[test]
    fn tilde_and_file_url_dirs_are_normalized_on_the_env_and_cli_tiers() {
        let home = PathBuf::from(FIXTURE_HOME);

        // The env tier arrives already expanded — `EnvVars::from_lookup` runs
        // `paths::cyrup_dir_override_from` against the home it resolved, so a `~` never reaches
        // this struct. Written expanded here for the same reason.
        let env = EnvVars {
            agent_dir: Some(home.join("alt")),
            ..env_rooted_at(FIXTURE_HOME)
        };
        let cli = CliConfigOverrides {
            cwd: Some(PathBuf::from("/")),
            ..Default::default()
        };
        let dirs = ConfigDirs::resolve(&cli, &env).unwrap();
        assert_eq!(dirs.agent_dir, home.join("alt"));

        let cli = CliConfigOverrides {
            cwd: Some(PathBuf::from("/")),
            session_dir: Some(PathBuf::from("~/sessions")),
            package_dir: Some(PathBuf::from("file:///abs/packages")),
            ..Default::default()
        };
        let dirs = ConfigDirs::resolve(&cli, &env_rooted_at(FIXTURE_HOME)).unwrap();
        assert_eq!(dirs.session_dir, home.join("sessions"));
        assert!(dirs.session_dir_explicit);
        assert_eq!(dirs.package_dir, PathBuf::from("/abs/packages"));
    }

    /// Tier 3 of Pi's `sessionDir` chain (main.ts:625-630): with neither `--session-dir` nor
    /// `$CYRUP_SESSION_DIR`, the merged `settings.json` key wins over the `<agent_dir>/sessions`
    /// default — and counts as EXPLICIT, because Pi hands it to `createSessionManager` in the same
    /// argument slot as the flag (main.ts:630), which selects the literal (not cwd-encoded) layout.
    #[test]
    fn settings_session_dir_overrides_the_default_and_is_explicit() {
        let env = env_rooted_at(FIXTURE_HOME);
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
        let env = env_rooted_at(FIXTURE_HOME);
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod telemetry_tristate_tests {
    use super::{CliConfigOverrides, EnvVars};
    use crate::policy::NetworkPolicy;
    use crate::settings::{EffectiveSettings, Settings};
    use std::ffi::OsString;

    fn env_with(pairs: &[(&str, &str)]) -> EnvVars {
        let owned: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), OsString::from(*v)))
            .collect();
        EnvVars::from_lookup(move |key| {
            owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        })
    }

    /// Settings that ask for install telemetry, so the env tier is the only thing that can say no.
    fn telemetry_on_settings() -> EffectiveSettings {
        EffectiveSettings::from_settings(
            Settings::parse(r#"{"enableInstallTelemetry": true}"#).expect("valid settings"),
        )
    }

    /// DRIFT-050: Pi's `isInstallTelemetryEnabled` (telemetry.ts:8-12 @v0.83.0) branches on
    /// `telemetryEnv !== undefined`, NOT on truthiness. `PI_TELEMETRY=` therefore takes the env
    /// branch and `isTruthyEnvFlag("")` (`:3-5`) returns false — an explicit OFF that overrides the
    /// settings value. cyrup filtered the empty string away, collapsing three states into two.
    #[test]
    fn drift050_a_set_but_empty_telemetry_var_is_an_explicit_off() {
        assert_eq!(env_with(&[]).telemetry, None, "unset defers to settings");
        assert_eq!(
            env_with(&[("CYRUP_TELEMETRY", "")]).telemetry,
            Some(false),
            "set-but-empty is an explicit OFF, not an absent value"
        );
        assert_eq!(env_with(&[("CYRUP_TELEMETRY", "1")]).telemetry, Some(true));
        // The alias carries the same tri-state.
        assert_eq!(env_with(&[("PI_TELEMETRY", "")]).telemetry, Some(false));
        assert_eq!(env_with(&[("PI_TELEMETRY", "true")]).telemetry, Some(true));
        // Pi's precedence: the first key that is SET wins, even when it is empty.
        assert_eq!(
            env_with(&[("CYRUP_TELEMETRY", ""), ("PI_TELEMETRY", "1")]).telemetry,
            Some(false)
        );
    }

    /// The tri-state has to survive all the way to the gate — `policy.rs:25-27`'s
    /// `env.telemetry.unwrap_or_else(|| s.enable_install_telemetry())`.
    #[test]
    fn drift050_an_empty_telemetry_var_beats_the_settings_opt_in() {
        let settings = telemetry_on_settings();
        let cli = CliConfigOverrides::default();

        let unset = NetworkPolicy::resolve(&settings, &env_with(&[]), &cli);
        assert!(unset.install_telemetry, "unset falls through to settings");

        let emptied =
            NetworkPolicy::resolve(&settings, &env_with(&[("CYRUP_TELEMETRY", "")]), &cli);
        assert!(
            !emptied.install_telemetry,
            "`CYRUP_TELEMETRY= cyrup …` must ship no install telemetry"
        );
        assert!(!emptied.allow_install_telemetry());

        let on = NetworkPolicy::resolve(&settings, &env_with(&[("CYRUP_TELEMETRY", "1")]), &cli);
        assert!(on.install_telemetry);
    }

    /// The two SIBLING flags are plain truthiness tests upstream (`main.ts:95-98`,
    /// `package-manager.ts:42-46`) and must NOT gain the tri-state.
    #[test]
    fn drift050_offline_and_skip_version_check_stay_two_state() {
        let emptied = env_with(&[
            ("CYRUP_OFFLINE", ""),
            ("CYRUP_SKIP_VERSION_CHECK", ""),
            ("CYRUP_CLEAR_ON_SHRINK", ""),
        ]);
        assert!(!emptied.offline);
        assert!(!emptied.skip_version_chk);
        assert!(!emptied.clear_on_shrink);
        // And an EMPTY first key must still fall through to a set second key for these, which is
        // exactly what the telemetry field must NOT do.
        let fallthrough = env_with(&[("CYRUP_OFFLINE", ""), ("PI_OFFLINE", "1")]);
        assert!(
            fallthrough.offline,
            "the non-tri-state fields keep Pi's per-key empty filter"
        );
    }
}
