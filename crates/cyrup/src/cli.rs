//! The clap argument surface and its mapping onto a [`SessionConfig`] + the runtime [`AppMode`]
//! (arch-11 §3.7/§6.1; R-11-001/024). A 1:1 port of Pi `cli/args.ts`: every flag Pi's hand-rolled
//! parser accepts is present here, with Pi's short aliases (incl. the multi-char `-nt`/`-nbt`/`-xt`/
//! `-ne`/`-ns`/`-np`/`-nc`/`-na` forms, which clap cannot express as native shorts — they are
//! rewritten to their long forms by [`normalize_short_aliases`] before parsing). Kept free of I/O so
//! the mapping is unit-testable.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use cyrup_config::{AppMode, ConfigDirs};
use cyrup_sdk::core::ModelThinkingLevel;
use cyrup_session_svc::{
    ExtensionFlagValue as SvcExtensionFlagValue, NoTools, SessionConfig, SessionTarget,
};

/// Pi's primary output selector `--mode <text|json|rpc>` (args.ts:78-82).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Mode {
    /// Human-oriented text (the default; interactive unless `--print`/non-TTY).
    Text,
    /// One `AgentSessionEvent` per line (JSONL).
    Json,
    /// The persistent stdin/stdout RPC line protocol.
    Rpc,
}

/// Output format for the non-interactive one-shot path (`--output-format`; a cyrup back-compat
/// alias — Pi expresses this through `--mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum OutputFormat {
    /// Human-oriented final assistant text (PRINT mode).
    Text,
    /// One `AgentSessionEvent` per line (JSON mode / JSONL).
    Json,
}

/// `--thinking <level>` (args.ts:57,130). Clap validates membership; the warning-on-invalid path Pi
/// takes (args.ts:135) is unreachable here because clap rejects an unknown value with a usage error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum ThinkingArg {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

impl ThinkingArg {
    /// Map onto the core [`ModelThinkingLevel`].
    pub fn to_level(self) -> ModelThinkingLevel {
        match self {
            ThinkingArg::Off => ModelThinkingLevel::Off,
            ThinkingArg::Minimal => ModelThinkingLevel::Minimal,
            ThinkingArg::Low => ModelThinkingLevel::Low,
            ThinkingArg::Medium => ModelThinkingLevel::Medium,
            ThinkingArg::High => ModelThinkingLevel::High,
            ThinkingArg::Xhigh => ModelThinkingLevel::Xhigh,
        }
    }
}

/// The cyrup command line (arch-11 §3.7; Pi `cli/args.ts`).
///
/// Mode precedence (R-11-001), resolved by [`resolve_app_mode`]: `--rpc`/`--mode rpc` ▷
/// `--json`/`--mode json` ▷ `--print` ▷ (no TTY) PRINT ▷ interactive TUI.
#[derive(Parser, Debug, Default)]
#[command(
    name = "cyrup",
    version,
    about = "cyrup — a Rust agent harness (a port of Pi)",
    disable_version_flag = true,
    disable_help_flag = true
)]
pub struct Cli {
    // ---- help/version (args.ts:74-77) ----
    /// Print the version and exit (`-v`, matching Pi; `--verbose` carries no short).
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    pub version: Option<bool>,
    /// Show the rich help body and exit (`-h`/`--help`). Pi prints its own hand-rolled help
    /// (args.ts:212), so clap's auto-help is disabled and [`render_help`] is used instead.
    #[arg(short = 'h', long = "help")]
    pub help: bool,

    // ---- mode selection (R-11-001; args.ts:78) ----
    /// Output mode: `text` (default), `json`, or `rpc`.
    #[arg(long = "mode", value_enum)]
    pub mode: Option<Mode>,
    /// One-shot PRINT mode: run to completion, print the final assistant text, exit.
    #[arg(short = 'p', long)]
    pub print: bool,
    /// One-shot output format: `text` (PRINT) or `json` (JSONL) — a cyrup back-compat alias.
    #[arg(long = "output-format", value_enum)]
    pub output_format: Option<OutputFormat>,
    /// Shorthand for `--mode json` (the JSONL `AgentSessionEvent` stream) — back-compat alias.
    #[arg(long)]
    pub json: bool,
    /// Shorthand for `--mode rpc` — back-compat alias.
    #[arg(long)]
    pub rpc: bool,

    // ---- provider / model (args.ts:87-92,130) ----
    /// Provider name (e.g. `openai`, `anthropic`); combines with `--model`.
    #[arg(long = "provider")]
    pub provider: Option<String>,
    /// Model selection pattern (`provider/id[:level]`).
    #[arg(long = "model")]
    pub model: Option<String>,
    /// Runtime API key for the selected provider (defaults to env vars).
    #[arg(long = "api-key")]
    pub api_key: Option<String>,
    /// Thinking level: off, minimal, low, medium, high, xhigh.
    #[arg(long = "thinking", value_enum)]
    pub thinking: Option<ThinkingArg>,
    /// Comma-separated model patterns for Ctrl+P cycling (globs/fuzzy/`:level`).
    #[arg(long = "models", value_delimiter = ',')]
    pub models: Vec<String>,

    // ---- prompt assembly (args.ts:93-97) ----
    /// Replace the assembled system prompt entirely.
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,
    /// Append text after the assembled system prompt (repeatable).
    #[arg(long = "append-system-prompt")]
    pub append_system_prompt: Vec<String>,

    // ---- tools (args.ts:116-129) ----
    /// Disable all tools by default (built-in and extension).
    #[arg(long = "no-tools")]
    pub no_tools: bool,
    /// Disable built-in tools by default but keep extension/custom tools enabled.
    #[arg(long = "no-builtin-tools")]
    pub no_builtin_tools: bool,
    /// Comma-separated allowlist of tool names to enable.
    #[arg(short = 't', long = "tools", value_delimiter = ',')]
    pub tools: Vec<String>,
    /// Comma-separated denylist of tool names to disable.
    #[arg(long = "exclude-tools", value_delimiter = ',')]
    pub exclude_tools: Vec<String>,

    // ---- resources (args.ts:149-170) ----
    /// Load an extension file (repeatable).
    #[arg(short = 'e', long = "extension")]
    pub extension: Vec<PathBuf>,
    /// Disable extension discovery (explicit `-e` paths still work).
    #[arg(long = "no-extensions")]
    pub no_extensions: bool,
    /// Load a skill file or directory (repeatable).
    #[arg(long = "skill")]
    pub skill: Vec<PathBuf>,
    /// Disable skills discovery and loading.
    #[arg(long = "no-skills")]
    pub no_skills: bool,
    /// Load a prompt template file or directory (repeatable).
    #[arg(long = "prompt-template")]
    pub prompt_template: Vec<PathBuf>,
    /// Disable prompt template discovery and loading.
    #[arg(long = "no-prompt-templates")]
    pub no_prompt_templates: bool,
    /// Load a theme file or directory (repeatable).
    #[arg(long = "theme")]
    pub theme: Vec<PathBuf>,
    /// Disable theme discovery and loading.
    #[arg(long = "no-themes")]
    pub no_themes: bool,
    /// Do not load `AGENTS.md`/`CLAUDE.md` context files.
    #[arg(long = "no-context-files")]
    pub no_context_files: bool,

    // ---- trust (arch-07 / R-11-029; args.ts:180-183) ----
    /// Trust the project for this run (`--approve`); enables trust-requiring resources.
    #[arg(short = 'a', long = "approve")]
    pub approve: bool,
    /// Refuse project trust for this run (`--no-approve`).
    #[arg(long = "no-approve")]
    pub no_approve: bool,

    // ---- session (args.ts:83,85,98-113) ----
    /// Continue the most recent session for this cwd.
    #[arg(short = 'c', long = "continue")]
    pub r#continue: bool,
    /// Select a session to resume (interactive picker).
    #[arg(short = 'r', long = "resume")]
    pub resume: bool,
    /// Use a specific session file or partial UUID.
    #[arg(long = "session")]
    pub session: Option<String>,
    /// Use the exact project session ID, creating it if missing.
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
    /// Fork a specific session file or partial UUID into a new session.
    #[arg(long = "fork")]
    pub fork: Option<String>,
    /// Directory for session storage and lookup.
    #[arg(long = "session-dir")]
    pub session_dir: Option<PathBuf>,
    /// Don't save the session (ephemeral).
    #[arg(long = "no-session")]
    pub no_session: bool,
    /// Set the session display name.
    #[arg(short = 'n', long = "name")]
    pub name: Option<String>,

    // ---- standalone actions (args.ts:147,171) ----
    /// Export a session file to HTML and exit (optional output path positional).
    #[arg(long = "export")]
    pub export: Option<PathBuf>,
    /// List available models (with optional fuzzy search) and exit.
    #[arg(long = "list-models", num_args = 0..=1, default_missing_value = "")]
    pub list_models: Option<String>,

    // ---- network / diagnostics (args.ts:178,184) ----
    /// Disable startup network operations (same as `PI_OFFLINE=1`).
    #[arg(long = "offline")]
    pub offline: bool,
    /// Force verbose startup (raises stderr log verbosity; never pollutes the protocol stream).
    #[arg(long = "verbose")]
    pub verbose: bool,

    /// The prompt: bare message words and `@file` references, merged with piped stdin (R-11-006/025).
    #[arg(value_name = "PROMPT")]
    pub positionals: Vec<String>,

    /// Unknown `--flag[=val]` tokens captured as potential extension flags (Pi `unknownFlags`,
    /// args.ts:188-201). Not parsed by clap — populated by [`partition_extension_flags`] before the
    /// clap parse and set on the struct afterwards. The downstream *consumption* (feeding these to
    /// loaded extensions via `applyExtensionFlagValues`) is the outer extension tier (ledgered).
    #[arg(skip)]
    pub extension_flags: Vec<ExtensionFlag>,
}

/// A captured unknown CLI flag (Pi `unknownFlags` map entry, args.ts:52-53). `Bool(true)` is a bare
/// `--flag`; `Str` is `--flag=value` or `--flag value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtFlagValue {
    Bool(bool),
    Str(String),
}

/// A captured unknown flag (its name without the leading `--`, plus its value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionFlag {
    pub name: String,
    pub value: ExtFlagValue,
}

impl Cli {
    /// `--approve` (Some(true)) / `--no-approve` (Some(false)) / neither (None). Approve wins if both.
    pub fn trust_override(&self) -> Option<bool> {
        if self.approve {
            Some(true)
        } else if self.no_approve {
            Some(false)
        } else {
            None
        }
    }

    /// The default tool-suppression mode (`--no-tools` ⇒ all; `--no-builtin-tools` ⇒ builtin; else
    /// none). `--no-tools` wins if both are given (it is strictly broader).
    pub fn no_tools_mode(&self) -> Option<NoTools> {
        if self.no_tools {
            Some(NoTools::All)
        } else if self.no_builtin_tools {
            Some(NoTools::Builtin)
        } else {
            None
        }
    }

    /// Which session the run targets. Resolution precedence mirrors Pi (main.ts:274-345):
    /// `--fork` ▷ `--session`/`--session-id` ▷ `--continue` ▷ new. A bare `--resume` is an
    /// interactive picker (resolved by the runtime), so it maps to `New` for the one-shot config.
    /// `base_session_dir` is the resolved sessions root used to turn a bare id into a file path.
    pub fn session_target(&self, base_session_dir: &std::path::Path) -> SessionTarget {
        if let Some(spec) = &self.fork {
            return SessionTarget::Resume(resolve_session_ref(spec, base_session_dir));
        }
        if let Some(spec) = self.session.as_ref().or(self.session_id.as_ref()) {
            return SessionTarget::Resume(resolve_session_ref(spec, base_session_dir));
        }
        if self.r#continue {
            return SessionTarget::Continue;
        }
        SessionTarget::New
    }

    /// Conflicting-session-flag diagnostics, a 1:1 port of Pi `validateForkFlags` (main.ts:205-219)
    /// and `validateSessionIdFlags` (main.ts:221-242). `--fork` conflicts with `--session`,
    /// `--continue`, `--resume`, `--no-session` (NOT `--session-id` — Pi forks into a new id);
    /// `--session-id` conflicts with `--session`, `--continue`, `--resume` and must pass
    /// [`assert_valid_session_id`]. The joined conflict list matches Pi's message exactly.
    pub fn validate_session_flags(&self) -> Result<(), String> {
        if self.fork.is_some() {
            let mut conflicts: Vec<&str> = Vec::new();
            if self.session.is_some() {
                conflicts.push("--session");
            }
            if self.r#continue {
                conflicts.push("--continue");
            }
            if self.resume {
                conflicts.push("--resume");
            }
            if self.no_session {
                conflicts.push("--no-session");
            }
            if !conflicts.is_empty() {
                return Err(format!(
                    "--fork cannot be combined with {}",
                    conflicts.join(", ")
                ));
            }
        }
        if let Some(id) = &self.session_id {
            let mut conflicts: Vec<&str> = Vec::new();
            if self.session.is_some() {
                conflicts.push("--session");
            }
            if self.r#continue {
                conflicts.push("--continue");
            }
            if self.resume {
                conflicts.push("--resume");
            }
            if !conflicts.is_empty() {
                return Err(format!(
                    "--session-id cannot be combined with {}",
                    conflicts.join(", ")
                ));
            }
            assert_valid_session_id(id)?;
        }
        Ok(())
    }

    /// The trimmed `--name`, erroring when empty after trim (Pi main.ts:586-592). `Ok(None)` when no
    /// `--name` was given.
    pub fn validated_name(&self) -> Result<Option<String>, String> {
        match &self.name {
            None => Ok(None),
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    Err("--name requires a non-empty value".to_string())
                } else {
                    Ok(Some(trimmed.to_string()))
                }
            }
        }
    }

    /// The `--append-system-prompt` parts joined (Pi keeps a `string[]`; the builder takes one
    /// blob). `None` when empty.
    pub fn append_system_prompt_joined(&self) -> Option<String> {
        if self.append_system_prompt.is_empty() {
            None
        } else {
            Some(self.append_system_prompt.join("\n"))
        }
    }

    /// Map the CLI + resolved directories + runtime mode onto a [`SessionConfig`] (arch-11 §3.7).
    ///
    /// Persistence (R-11-008): one-shot PRINT/JSON default to an ephemeral in-memory session unless a
    /// session is explicitly resumed/continued; interactive always persists. `--no-session` forces
    /// ephemeral in every mode (Pi `noSession`, args.ts:104).
    pub fn to_session_config(&self, dirs: &ConfigDirs, mode: AppMode) -> SessionConfig {
        let mut config = SessionConfig::new(dirs.cwd.clone(), dirs.agent_dir.clone());
        // Thread the REAL user home (not the agent dir) so the resources ancestor-walk dedup
        // (`~/.agents/skills`) and the trust-requiring-resource walk resolve against `$HOME`, exactly
        // like Pi's `getHomeDir()` (`process.env.HOME || homedir()`, package-manager.ts:217) and
        // trust-manager.ts:185. `SessionConfig::new` defaults `home` to the agent dir; override it here.
        config.home = dirs.home.clone();
        config.session_dir = Some(dirs.session_dir.clone());
        config.app_mode = mode;
        config.model_pattern = self.model.clone();
        // An explicit `--provider` lets the builder's custom-fallback fire for a bare unresolvable
        // `--model` id (Pi `cliProvider`, model-resolver.ts:369,475).
        config.cli_provider_explicit = self.provider.is_some();
        config.thinking_level = self.thinking.map(ThinkingArg::to_level);
        config.trust_override = self.trust_override();
        config.no_context_files = self.no_context_files;
        config.no_skills = self.no_skills;
        config.no_prompt_templates = self.no_prompt_templates;
        config.no_themes = self.no_themes;
        // `--no-extensions`/`-ne` disables extension discovery; explicit `--extension`/`-e` paths still
        // load (Pi `resourceLoaderOptions.noExtensions`/`additionalExtensionPaths`, main.ts:660,664).
        config.no_extensions = self.no_extensions;
        config.extra_extension_paths = resolve_cli_paths(&dirs.cwd, &self.extension);
        // Relative resource paths are resolved to absolute vs the cwd before threading (Pi
        // `resolveCliPaths`, main.ts:450-451,605-608); package-source specs (npm:/git:/…) are kept.
        config.extra_skill_paths = resolve_cli_paths(&dirs.cwd, &self.skill);
        config.extra_prompt_paths = resolve_cli_paths(&dirs.cwd, &self.prompt_template);
        config.extra_theme_paths = resolve_cli_paths(&dirs.cwd, &self.theme);
        config.system_prompt = self.system_prompt.clone();
        config.append_system_prompt = self.append_system_prompt_joined();
        config.no_tools = self.no_tools_mode();
        if !self.tools.is_empty() {
            config.tools = Some(self.tools.clone());
        }
        config.exclude_tools = self.exclude_tools.clone();
        // Thread the captured extension flags (Pi `extensionFlagValues: parsed.unknownFlags`,
        // main.ts:634) onto the config so they reach the session services; a loaded extension reads
        // them via `applyExtensionFlagValues` (the WASM-guest consumption is the ext-host tier).
        config.extension_flag_values = self
            .extension_flags
            .iter()
            .map(|f| {
                let value = match &f.value {
                    ExtFlagValue::Bool(b) => SvcExtensionFlagValue::Bool(*b),
                    ExtFlagValue::Str(s) => SvcExtensionFlagValue::Str(s.clone()),
                };
                (f.name.clone(), value)
            })
            .collect();
        config.target = self.session_target(&dirs.session_dir);
        let explicit_session = matches!(
            config.target,
            SessionTarget::Resume(_) | SessionTarget::Continue
        );
        config.persist = !self.no_session && (explicit_session || mode == AppMode::Interactive);
        config
    }
}

/// Resolve a `--session`/`--session-id`/`--fork` reference (a path or a bare id) to a session file
/// path (Pi `resolveSessionPath`, main.ts:297). An existing path is used verbatim; otherwise the id
/// is joined under `base_session_dir` with the `.jsonl` extension Pi uses for session files.
fn resolve_session_ref(spec: &str, base_session_dir: &std::path::Path) -> PathBuf {
    let as_path = PathBuf::from(spec);
    if as_path.exists() || spec.contains(std::path::MAIN_SEPARATOR) {
        return as_path;
    }
    let file = if spec.ends_with(".jsonl") {
        spec.to_string()
    } else {
        format!("{spec}.jsonl")
    };
    base_session_dir.join(file)
}

/// Split a `--models` pattern into its base and optional `:level` thinking suffix (Pi
/// `resolveModelScope`, main.ts:685): `sonnet:high` ⇒ `("sonnet", Some(High))`. Only a trailing,
/// recognized level is treated as a suffix (so `provider/id` slashes are preserved).
pub fn split_model_level(pattern: &str) -> (String, Option<ThinkingArg>) {
    if let Some((base, level)) = pattern.rsplit_once(':')
        && let Ok(parsed) = ThinkingArg::from_str(level, true)
    {
        return (base.to_string(), Some(parsed));
    }
    (pattern.to_string(), None)
}

/// Does a `provider/id`-qualified model name match a base `--models` pattern? Supports `*` globs
/// (`anthropic/*`, `*sonnet*`) and a case-insensitive substring fallback (Pi's fuzzy scope match).
pub fn qualified_matches(qualified: &str, base_pattern: &str) -> bool {
    let q = qualified.to_ascii_lowercase();
    let p = base_pattern.to_ascii_lowercase();
    if p.contains('*') {
        glob_match(&q, &p)
    } else {
        q == p || q.contains(&p)
    }
}

/// A minimal `*`-glob matcher (each `*` matches any run, including empty); no `?`/char-classes.
fn glob_match(text: &str, pattern: &str) -> bool {
    let segments: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0usize;
    for (i, seg) in segments.iter().enumerate() {
        if seg.is_empty() {
            continue;
        }
        match text[pos..].find(seg) {
            Some(found) => {
                // The first non-empty segment must anchor at the start unless the pattern began `*`.
                if i == 0 && found != 0 {
                    return false;
                }
                pos += found + seg.len();
            }
            None => return false,
        }
    }
    // A trailing non-`*` segment must reach the end of the text.
    match pattern.rsplit('*').next() {
        Some(last) if !last.is_empty() => text.ends_with(last),
        _ => true,
    }
}

/// Rewrite Pi's multi-character short flags (`-nt`/`-nbt`/`-xt`/`-ne`/`-ns`/`-np`/`-nc`/`-na`) to
/// their long forms before clap parsing — clap's native shorts are single-character only, so these
/// Pi aliases (args.ts:116-183) are normalized here so `cyrup -nt` is accepted exactly as Pi accepts
/// it. Only exact whole-token matches are rewritten; longer combinations are left untouched.
pub fn normalize_short_aliases<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    args.into_iter()
        .map(Into::into)
        .map(|a| match a.as_str() {
            "-nt" => "--no-tools".to_string(),
            "-nbt" => "--no-builtin-tools".to_string(),
            "-xt" => "--exclude-tools".to_string(),
            "-ne" => "--no-extensions".to_string(),
            "-ns" => "--no-skills".to_string(),
            "-np" => "--no-prompt-templates".to_string(),
            "-nc" => "--no-context-files".to_string(),
            "-na" => "--no-approve".to_string(),
            _ => a,
        })
        .collect()
}

/// Resolve the runtime mode (R-11-001 / arch-11 §6.1). Explicit mode flags win; otherwise a
/// non-TTY stdin or stdout forces PRINT, and a full TTY pair selects the interactive front-end.
/// `--mode text` is the DEFAULT (it does not force PRINT by itself — only `--print`/non-TTY does).
pub fn resolve_app_mode(cli: &Cli, stdin_tty: bool, stdout_tty: bool) -> AppMode {
    if cli.rpc || cli.mode == Some(Mode::Rpc) {
        return AppMode::Rpc;
    }
    if cli.json || cli.mode == Some(Mode::Json) || cli.output_format == Some(OutputFormat::Json) {
        return AppMode::Json;
    }
    if cli.print || cli.output_format == Some(OutputFormat::Text) {
        return AppMode::Print;
    }
    if !stdin_tty || !stdout_tty {
        return AppMode::Print;
    }
    AppMode::Interactive
}

/// Validate a `--session-id` against Pi's id grammar (`assertValidSessionId`, session-manager.ts:207):
/// `^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$` — non-empty, only alphanumerics + `-`/`_`/`.`, and
/// alphanumeric at both ends. Returns Pi's exact error message on failure.
pub fn assert_valid_session_id(id: &str) -> Result<(), String> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    if valid {
        Ok(())
    } else {
        Err("Session id must be non-empty, contain only alphanumeric characters, '-', '_', and '.', \
             and start and end with an alphanumeric character"
            .to_string())
    }
}

/// Is `value` a local filesystem path (vs a package-source spec)? Port of Pi `isLocalPath`
/// (paths.ts): `npm:`/`git:`/`github:`/`http:`/`https:`/`ssh:`-prefixed specs are NOT local.
pub fn is_local_path(value: &str) -> bool {
    let t = value.trim();
    !(t.starts_with("npm:")
        || t.starts_with("git:")
        || t.starts_with("github:")
        || t.starts_with("http:")
        || t.starts_with("https:")
        || t.starts_with("ssh:"))
}

/// Resolve relative CLI resource paths to absolute against `cwd`, leaving package-source specs alone
/// (Pi `resolveCliPaths`, main.ts:450-451). An already-absolute local path is kept verbatim.
pub fn resolve_cli_paths(cwd: &std::path::Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|p| {
            let s = p.to_string_lossy();
            if is_local_path(&s) && !p.is_absolute() {
                cwd.join(p)
            } else {
                p.clone()
            }
        })
        .collect()
}

/// `isPlainRuntimeMetadataCommand` (main.ts:117-119): a non-`--print`, no-`--mode`, `--help`-or-
/// `--list-models` invocation. Such commands keep stdout pristine for their own output (they are NOT
/// stdout-guarded).
pub fn is_plain_runtime_metadata_command(cli: &Cli) -> bool {
    !cli.print && cli.mode.is_none() && (cli.help || cli.list_models.is_some())
}

/// `shouldTakeOverStdout` (main.ts:535): take over stdout for non-interactive modes that are not a
/// plain metadata command, so stray library writes cannot pollute the PRINT/JSON/RPC stream. (In
/// Rust the streams the bin owns are already disciplined — tracing goes to stderr — so the takeover
/// itself is a no-op; only the Pi-faithful DECISION is modelled and tested here.)
pub fn should_take_over_stdout(cli: &Cli, mode: AppMode) -> bool {
    mode != AppMode::Interactive && !is_plain_runtime_metadata_command(cli)
}

/// Partition `argv` (program name already stripped, short-aliases already normalized) into the args
/// clap should parse and the captured unknown `--flag[=val]` extension flags — a 1:1 port of Pi's
/// hand-rolled unknown-flag arm (args.ts:188-201). A `--flag=val` captures `(flag,val)`; a bare
/// `--flag` followed by a non-`-`/non-`@` token captures `(flag,next)` and consumes it, else captures
/// `(flag,true)`. Values of KNOWN value-taking long flags are passed through untouched (so `--model
/// --x` is not mis-captured). Single-dash unknowns are left for clap (it reports them like Pi's
/// "Unknown option" diagnostic).
pub fn partition_extension_flags(argv: &[String]) -> (Vec<String>, Vec<ExtensionFlag>) {
    let mut clean: Vec<String> = Vec::new();
    let mut flags: Vec<ExtensionFlag> = Vec::new();
    let mut i = 0usize;
    while let Some(arg) = argv.get(i) {
        let name_part = arg.split('=').next().unwrap_or(arg);
        if let Some(stripped) = arg.strip_prefix("--") {
            if KNOWN_LONG_FLAGS.contains(&name_part) {
                clean.push(arg.clone());
                // A known value-taking flag in its space-separated form consumes the next token.
                if KNOWN_VALUE_LONG_FLAGS.contains(&name_part)
                    && !arg.contains('=')
                    && let Some(next) = argv.get(i + 1)
                {
                    clean.push(next.clone());
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }
            // Unknown long flag → capture as an extension flag (Pi args.ts:188-201).
            if let Some(eq) = stripped.find('=') {
                flags.push(ExtensionFlag {
                    name: stripped[..eq].to_string(),
                    value: ExtFlagValue::Str(stripped[eq + 1..].to_string()),
                });
                i += 1;
                continue;
            }
            match argv.get(i + 1) {
                Some(next) if !next.starts_with('-') && !next.starts_with('@') => {
                    flags.push(ExtensionFlag {
                        name: stripped.to_string(),
                        value: ExtFlagValue::Str(next.clone()),
                    });
                    i += 2;
                }
                _ => {
                    flags.push(ExtensionFlag {
                        name: stripped.to_string(),
                        value: ExtFlagValue::Bool(true),
                    });
                    i += 1;
                }
            }
            continue;
        }
        clean.push(arg.clone());
        i += 1;
    }
    (clean, flags)
}

/// Every long flag clap knows (used by [`partition_extension_flags`] to leave known flags + their
/// values for clap). Kept in lockstep with the [`Cli`] struct.
const KNOWN_LONG_FLAGS: &[&str] = &[
    "--version",
    "--help",
    "--mode",
    "--print",
    "--output-format",
    "--json",
    "--rpc",
    "--provider",
    "--model",
    "--api-key",
    "--thinking",
    "--models",
    "--system-prompt",
    "--append-system-prompt",
    "--no-tools",
    "--no-builtin-tools",
    "--tools",
    "--exclude-tools",
    "--extension",
    "--no-extensions",
    "--skill",
    "--no-skills",
    "--prompt-template",
    "--no-prompt-templates",
    "--theme",
    "--no-themes",
    "--no-context-files",
    "--approve",
    "--no-approve",
    "--continue",
    "--resume",
    "--session",
    "--session-id",
    "--fork",
    "--session-dir",
    "--no-session",
    "--name",
    "--export",
    "--list-models",
    "--offline",
    "--verbose",
];

/// The subset of [`KNOWN_LONG_FLAGS`] that take a value in their space-separated form (so the next
/// token must be passed through to clap, never captured as an extension flag). `--list-models` is
/// intentionally excluded — its value is optional and clap resolves it.
const KNOWN_VALUE_LONG_FLAGS: &[&str] = &[
    "--mode",
    "--output-format",
    "--provider",
    "--model",
    "--api-key",
    "--thinking",
    "--models",
    "--system-prompt",
    "--append-system-prompt",
    "--tools",
    "--exclude-tools",
    "--extension",
    "--skill",
    "--prompt-template",
    "--theme",
    "--session",
    "--session-id",
    "--fork",
    "--session-dir",
    "--name",
    "--export",
];

/// Render Pi's rich `--help` body (args.ts:212-389): usage, the package/config commands, the full
/// option list, the registered-extension-flag block, examples, the environment-variable catalogue,
/// and the built-in tool names. `extension_flags` are the flags loaded extensions registered; the bin
/// passes an empty slice today (the loaded-extension flag tier is the outer extension layer,
/// ledgered), but the injection point is preserved 1:1.
pub fn render_help(extension_flags: &[ExtensionFlag]) -> String {
    const APP: &str = "cyrup";
    const CFG: &str = ".cyrup";
    const ENV_AGENT_DIR: &str = "CYRUP_AGENT_DIR";
    const ENV_SESSION_DIR: &str = "CYRUP_SESSION_DIR";
    let ext_block = if extension_flags.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = extension_flags
            .iter()
            .map(|f| {
                let value = if matches!(f.value, ExtFlagValue::Str(_)) {
                    " <value>"
                } else {
                    ""
                };
                format!("  --{}{}", f.name, value)
            })
            .collect();
        format!("\nExtension CLI Flags:\n{}\n", lines.join("\n"))
    };
    format!(
        "{APP} - AI coding assistant with read, bash, edit, write tools

Usage:
  {APP} [options] [@files...] [messages...]

Commands:
  {APP} install <source> [-l]     Install extension source and add to settings
  {APP} remove <source> [-l]      Remove extension source from settings
  {APP} uninstall <source> [-l]   Alias for remove
  {APP} update [source|self|pi]   Update {APP} (use --all for {APP} and extensions)
  {APP} list                      List installed extensions from settings
  {APP} config                    Open TUI to enable/disable package resources
  {APP} <command> --help          Show help for install/remove/uninstall/update/list

Options:
  --provider <name>              Provider name (default: google)
  --model <pattern>              Model pattern or ID (supports \"provider/id\" and optional \":<thinking>\")
  --api-key <key>                API key (defaults to env vars)
  --system-prompt <text>         System prompt (default: coding assistant prompt)
  --append-system-prompt <text>  Append text or file contents to the system prompt (can be used multiple times)
  --mode <mode>                  Output mode: text (default), json, or rpc
  --print, -p                    Non-interactive mode: process prompt and exit
  --continue, -c                 Continue previous session
  --resume, -r                   Select a session to resume
  --session <path|id>            Use specific session file or partial UUID
  --session-id <id>              Use exact project session ID, creating it if missing
  --fork <path|id>               Fork specific session file or partial UUID into a new session
  --session-dir <dir>            Directory for session storage and lookup
  --no-session                   Don't save session (ephemeral)
  --name, -n <name>              Set session display name
  --models <patterns>            Comma-separated model patterns for Ctrl+P cycling
                                 Supports globs (anthropic/*, *sonnet*) and fuzzy matching
  --no-tools, -nt                Disable all tools by default (built-in and extension)
  --no-builtin-tools, -nbt       Disable built-in tools by default but keep extension/custom tools enabled
  --tools, -t <tools>            Comma-separated allowlist of tool names to enable
                                 Applies to built-in, extension, and custom tools
  --exclude-tools, -xt <tools>   Comma-separated denylist of tool names to disable
                                 Applies to built-in, extension, and custom tools
  --thinking <level>             Set thinking level: off, minimal, low, medium, high, xhigh
  --extension, -e <path>         Load an extension file (can be used multiple times)
  --no-extensions, -ne           Disable extension discovery (explicit -e paths still work)
  --skill <path>                 Load a skill file or directory (can be used multiple times)
  --no-skills, -ns               Disable skills discovery and loading
  --prompt-template <path>       Load a prompt template file or directory (can be used multiple times)
  --no-prompt-templates, -np     Disable prompt template discovery and loading
  --theme <path>                 Load a theme file or directory (can be used multiple times)
  --no-themes                    Disable theme discovery and loading
  --no-context-files, -nc        Disable AGENTS.md and CLAUDE.md discovery and loading
  --export <file>                Export session file to HTML and exit
  --list-models [search]         List available models (with optional fuzzy search)
  --verbose                      Force verbose startup (overrides quietStartup setting)
  --approve, -a                  Trust project-local files for this run
  --no-approve, -na              Ignore project-local files for this run
  --offline                      Disable startup network operations (same as CYRUP_OFFLINE=1)
  --help, -h                     Show this help
  --version, -v                  Show version number

Extensions can register additional flags (e.g., --plan from plan-mode extension).{ext_block}

Examples:
  # Interactive mode
  {APP}

  # Interactive mode with initial prompt
  {APP} \"List all .ts files in src/\"

  # Include files in initial message
  {APP} @prompt.md @image.png \"What color is the sky?\"

  # Non-interactive mode (process and exit)
  {APP} -p \"List all .ts files in src/\"

  # Multiple messages (interactive)
  {APP} \"Read package.json\" \"What dependencies do we have?\"

  # Continue previous session
  {APP} --continue \"What did we discuss?\"

  # Start a named session
  {APP} --name \"Refactor auth module\"

  # Use different model
  {APP} --provider openai --model gpt-4o-mini \"Help me refactor this code\"

  # Use model with provider prefix (no --provider needed)
  {APP} --model openai/gpt-4o \"Help me refactor this code\"

  # Use model with thinking level shorthand
  {APP} --model sonnet:high \"Solve this complex problem\"

  # Limit model cycling to specific models
  {APP} --models claude-sonnet,claude-haiku,gpt-4o

  # Limit to a specific provider with glob pattern
  {APP} --models \"github-copilot/*\"

  # Cycle models with fixed thinking levels
  {APP} --models sonnet:high,haiku:low

  # Start with a specific thinking level
  {APP} --thinking high \"Solve this complex problem\"

  # Read-only mode (no file modifications possible)
  {APP} --tools read,grep,find,ls -p \"Review the code in src/\"

  # Disable one tool while keeping the rest available
  {APP} --exclude-tools ask_question

  # Export a session file to HTML
  {APP} --export ~/{CFG}/agent/sessions/--path--/session.jsonl
  {APP} --export session.jsonl output.html

Environment Variables:
  ANTHROPIC_API_KEY                - Anthropic Claude API key
  ANTHROPIC_OAUTH_TOKEN            - Anthropic OAuth token (alternative to API key)
  ANT_LING_API_KEY                 - Ant Ling API key
  OPENAI_API_KEY                   - OpenAI GPT API key
  AZURE_OPENAI_API_KEY             - Azure OpenAI API key
  AZURE_OPENAI_BASE_URL            - Azure OpenAI/Cognitive Services base URL (e.g. https://{{resource}}.openai.azure.com)
  AZURE_OPENAI_RESOURCE_NAME       - Azure OpenAI resource name (alternative to base URL)
  AZURE_OPENAI_API_VERSION         - Azure OpenAI API version (default: v1)
  AZURE_OPENAI_DEPLOYMENT_NAME_MAP - Azure OpenAI model=deployment map (comma-separated)
  DEEPSEEK_API_KEY                 - DeepSeek API key
  NVIDIA_API_KEY                   - NVIDIA NIM API key
  GEMINI_API_KEY                   - Google Gemini API key
  GROQ_API_KEY                     - Groq API key
  CEREBRAS_API_KEY                 - Cerebras API key
  XAI_API_KEY                      - xAI Grok API key
  FIREWORKS_API_KEY                - Fireworks API key
  TOGETHER_API_KEY                 - Together AI API key
  OPENROUTER_API_KEY               - OpenRouter API key
  AI_GATEWAY_API_KEY               - Vercel AI Gateway API key
  ZAI_API_KEY                      - ZAI Coding Plan API key (Global)
  ZAI_CODING_CN_API_KEY            - ZAI Coding Plan API key (China)
  MISTRAL_API_KEY                  - Mistral API key
  MINIMAX_API_KEY                  - MiniMax API key
  MOONSHOT_API_KEY                 - Moonshot AI API key
  OPENCODE_API_KEY                 - OpenCode Zen/OpenCode Go API key
  KIMI_API_KEY                     - Kimi For Coding API key
  CLOUDFLARE_API_KEY               - Cloudflare API token (Workers AI and AI Gateway)
  CLOUDFLARE_ACCOUNT_ID            - Cloudflare account id (required for both)
  CLOUDFLARE_GATEWAY_ID            - Cloudflare AI Gateway slug (required for AI Gateway)
  AWS_PROFILE                      - AWS profile for Amazon Bedrock
  AWS_ACCESS_KEY_ID                - AWS access key for Amazon Bedrock
  AWS_SECRET_ACCESS_KEY            - AWS secret key for Amazon Bedrock
  AWS_BEARER_TOKEN_BEDROCK         - Bedrock API key (bearer token)
  AWS_REGION                       - AWS region for Amazon Bedrock (e.g., us-east-1)
  {ENV_AGENT_DIR:<32} - Config directory (default: ~/{CFG}/agent)
  {ENV_SESSION_DIR:<32} - Session storage directory (overridden by --session-dir)
  CYRUP_PACKAGE_DIR                - Override package directory (for Nix/Guix store paths)
  CYRUP_OFFLINE                    - Disable startup network operations when set to 1/true/yes
  CYRUP_TELEMETRY                  - Override install telemetry when set to 1/true/yes or 0/false/no
  CYRUP_SHARE_VIEWER_URL           - Base URL for /share command

Built-in Tool Names:
  read   - Read file contents
  bash   - Execute bash commands
  edit   - Edit files with find/replace
  write  - Write files (creates/overwrites)
  grep   - Search file contents (read-only, off by default)
  find   - Find files by glob pattern (read-only, off by default)
  ls     - List directory contents (read-only, off by default)
"
    )
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        let mut full = vec!["cyrup".to_string()];
        full.extend(normalize_short_aliases(args.iter().map(|s| s.to_string())));
        Cli::try_parse_from(full).expect("parse")
    }

    fn dirs() -> ConfigDirs {
        ConfigDirs {
            agent_dir: "/agent".into(),
            session_dir: "/agent/sessions".into(),
            package_dir: "/agent/packages".into(),
            cwd: "/work".into(),
            home: "/home/user".into(),
        }
    }

    #[test]
    fn mode_flag_and_aliases_take_precedence_over_tty() {
        assert_eq!(
            resolve_app_mode(&parse(&["--mode", "rpc"]), true, true),
            AppMode::Rpc
        );
        assert_eq!(
            resolve_app_mode(&parse(&["--rpc"]), true, true),
            AppMode::Rpc
        );
        assert_eq!(
            resolve_app_mode(&parse(&["--mode", "json"]), true, true),
            AppMode::Json
        );
        assert_eq!(
            resolve_app_mode(&parse(&["--json"]), true, true),
            AppMode::Json
        );
        assert_eq!(
            resolve_app_mode(&parse(&["-p"]), true, true),
            AppMode::Print
        );
        // `--mode text` is the default — interactive with a full TTY.
        assert_eq!(
            resolve_app_mode(&parse(&["--mode", "text"]), true, true),
            AppMode::Interactive
        );
    }

    #[test]
    fn tty_probing_selects_interactive_or_print() {
        let cli = parse(&[]);
        assert_eq!(resolve_app_mode(&cli, true, true), AppMode::Interactive);
        assert_eq!(resolve_app_mode(&cli, false, true), AppMode::Print);
        assert_eq!(resolve_app_mode(&cli, true, false), AppMode::Print);
    }

    #[test]
    fn version_short_is_v_not_verbose() {
        // `-v` is version (Pi args.ts:76); it triggers clap's Version action (an Err on try_parse).
        let full = vec!["cyrup".to_string(), "-v".to_string()];
        let err = Cli::try_parse_from(full).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
        // `--verbose` is a distinct boolean with no short.
        assert!(parse(&["--verbose"]).verbose);
    }

    #[test]
    fn trust_override_maps_approve_flags() {
        assert_eq!(parse(&["--approve"]).trust_override(), Some(true));
        assert_eq!(parse(&["--no-approve"]).trust_override(), Some(false));
        assert_eq!(parse(&["-na"]).trust_override(), Some(false));
        assert_eq!(parse(&[]).trust_override(), None);
        assert_eq!(
            parse(&["--approve", "--no-approve"]).trust_override(),
            Some(true)
        );
    }

    #[test]
    fn multi_char_short_aliases_normalize_to_longs() {
        assert!(parse(&["-nt"]).no_tools);
        assert!(parse(&["-nbt"]).no_builtin_tools);
        assert_eq!(
            parse(&["-xt", "ask"]).exclude_tools,
            vec!["ask".to_string()]
        );
        assert!(parse(&["-ne"]).no_extensions);
        assert!(parse(&["-ns"]).no_skills);
        assert!(parse(&["-np"]).no_prompt_templates);
        assert!(parse(&["-nc"]).no_context_files);
    }

    #[test]
    fn tool_flags_map_to_no_tools_modes_and_lists() {
        assert_eq!(parse(&["--no-tools"]).no_tools_mode(), Some(NoTools::All));
        assert_eq!(
            parse(&["--no-builtin-tools"]).no_tools_mode(),
            Some(NoTools::Builtin)
        );
        // --no-tools wins when both are present.
        assert_eq!(
            parse(&["--no-tools", "--no-builtin-tools"]).no_tools_mode(),
            Some(NoTools::All)
        );
        let cli = parse(&["--tools", "read,grep,find", "--exclude-tools", "bash"]);
        assert_eq!(
            cli.tools,
            vec!["read".to_string(), "grep".to_string(), "find".to_string()]
        );
        assert_eq!(cli.exclude_tools, vec!["bash".to_string()]);
    }

    #[test]
    fn provider_api_key_thinking_and_models_parse() {
        let cli = parse(&[
            "--provider",
            "openai",
            "--model",
            "openai/gpt-4o",
            "--api-key",
            "sk-test",
            "--thinking",
            "high",
            "--models",
            "claude-sonnet,gpt-4o:low",
        ]);
        assert_eq!(cli.provider.as_deref(), Some("openai"));
        assert_eq!(cli.model.as_deref(), Some("openai/gpt-4o"));
        assert_eq!(cli.api_key.as_deref(), Some("sk-test"));
        assert_eq!(cli.thinking, Some(ThinkingArg::High));
        assert_eq!(
            cli.models,
            vec!["claude-sonnet".to_string(), "gpt-4o:low".to_string()]
        );
    }

    #[test]
    fn resource_flags_repeat_and_negate() {
        let cli = parse(&[
            "--extension",
            "a.ts",
            "-e",
            "b.ts",
            "--skill",
            "s1",
            "--theme",
            "t1",
            "--prompt-template",
            "p1",
            "--no-themes",
        ]);
        assert_eq!(
            cli.extension,
            vec![PathBuf::from("a.ts"), PathBuf::from("b.ts")]
        );
        assert_eq!(cli.skill, vec![PathBuf::from("s1")]);
        assert_eq!(cli.theme, vec![PathBuf::from("t1")]);
        assert_eq!(cli.prompt_template, vec![PathBuf::from("p1")]);
        assert!(cli.no_themes);
    }

    #[test]
    fn list_models_optional_search_and_export() {
        assert_eq!(parse(&["--list-models"]).list_models.as_deref(), Some(""));
        assert_eq!(
            parse(&["--list-models", "sonnet"]).list_models.as_deref(),
            Some("sonnet")
        );
        assert_eq!(parse(&[]).list_models, None);
        assert_eq!(
            parse(&["--export", "s.jsonl"]).export,
            Some(PathBuf::from("s.jsonl"))
        );
    }

    #[test]
    fn session_target_precedence_and_validation() {
        let d = dirs();
        assert!(matches!(
            parse(&["-c"]).session_target(&d.session_dir),
            SessionTarget::Continue
        ));
        assert!(matches!(
            parse(&[]).session_target(&d.session_dir),
            SessionTarget::New
        ));
        // A bare id resolves under the session dir with `.jsonl`.
        match parse(&["--session", "abc123"]).session_target(&d.session_dir) {
            SessionTarget::Resume(p) => {
                assert_eq!(p, PathBuf::from("/agent/sessions/abc123.jsonl"))
            }
            other => panic!("expected resume, got {other:?}"),
        }
        // --fork wins over --continue (and conflicts are reported).
        assert!(
            parse(&["--fork", "x", "--continue"])
                .validate_session_flags()
                .is_err()
        );
        assert!(
            parse(&["--session", "a", "--session-id", "valid"])
                .validate_session_flags()
                .is_err()
        );
        // `--no-session --continue` is NOT a conflict in Pi (no-session just goes in-memory).
        assert!(
            parse(&["--no-session", "--continue"])
                .validate_session_flags()
                .is_ok()
        );
        // `--fork --session-id` is allowed (fork into a new id, Pi createSessionManager).
        assert!(
            parse(&["--fork", "x", "--session-id", "newid"])
                .validate_session_flags()
                .is_ok()
        );
        assert!(parse(&["--continue"]).validate_session_flags().is_ok());
    }

    #[test]
    fn session_id_format_is_validated() {
        assert!(assert_valid_session_id("abc-123_x.y").is_ok());
        assert!(assert_valid_session_id("a").is_ok());
        assert!(assert_valid_session_id("").is_err());
        assert!(assert_valid_session_id("-bad").is_err());
        assert!(assert_valid_session_id("bad-").is_err());
        assert!(assert_valid_session_id("bad/slash").is_err());
        // Threaded through the flag validator (a value clap accepts but the grammar rejects).
        assert!(
            parse(&["--session-id", "bad."])
                .validate_session_flags()
                .is_err()
        );
        assert!(
            parse(&["--session-id", "ok.id-1"])
                .validate_session_flags()
                .is_ok()
        );
    }

    #[test]
    fn name_is_trimmed_and_empty_is_rejected() {
        assert_eq!(
            parse(&["--name", "  hi  "])
                .validated_name()
                .unwrap()
                .as_deref(),
            Some("hi")
        );
        assert!(parse(&["--name", "   "]).validated_name().is_err());
        assert_eq!(parse(&[]).validated_name().unwrap(), None);
    }

    #[test]
    fn relative_resource_paths_resolve_to_absolute_keeping_specs() {
        let cwd = std::path::Path::new("/work");
        let out = resolve_cli_paths(
            cwd,
            &[
                PathBuf::from("rel/x.ts"),
                PathBuf::from("/abs/y.ts"),
                PathBuf::from("npm:@a/b"),
            ],
        );
        assert_eq!(out[0], PathBuf::from("/work/rel/x.ts"));
        assert_eq!(out[1], PathBuf::from("/abs/y.ts"));
        assert_eq!(out[2], PathBuf::from("npm:@a/b"));
    }

    #[test]
    fn unknown_flags_are_captured_as_extension_flags() {
        // `--plan` bare, `--mode=k=v` style with `=`, and a value form; known flags + their values
        // pass through to clap untouched.
        let (clean, flags) = partition_extension_flags(&[
            "--plan".to_string(),
            "--model".to_string(),
            "openai/gpt-4o".to_string(),
            "--reviewer=alice".to_string(),
            "--limit".to_string(),
            "5".to_string(),
            "hello".to_string(),
        ]);
        assert_eq!(
            clean,
            vec![
                "--model".to_string(),
                "openai/gpt-4o".to_string(),
                "hello".to_string()
            ]
        );
        assert_eq!(
            flags,
            vec![
                ExtensionFlag {
                    name: "plan".into(),
                    value: ExtFlagValue::Bool(true)
                },
                ExtensionFlag {
                    name: "reviewer".into(),
                    value: ExtFlagValue::Str("alice".into())
                },
                ExtensionFlag {
                    name: "limit".into(),
                    value: ExtFlagValue::Str("5".into())
                },
            ]
        );
        // The clean argv still parses under clap with the unknowns removed.
        let mut full = vec!["cyrup".to_string()];
        full.extend(clean);
        let cli = Cli::try_parse_from(full).expect("clean argv parses");
        assert_eq!(cli.model.as_deref(), Some("openai/gpt-4o"));
        assert_eq!(cli.positionals, vec!["hello".to_string()]);
    }

    #[test]
    fn stdout_takeover_decision_matches_pi() {
        // Plain metadata commands (help / list-models without --print/--mode) are NOT guarded.
        assert!(is_plain_runtime_metadata_command(&parse(&["--help"])));
        assert!(is_plain_runtime_metadata_command(&parse(&[
            "--list-models"
        ])));
        assert!(!is_plain_runtime_metadata_command(&parse(&[
            "-p",
            "--list-models"
        ])));
        // Print/JSON/RPC (non-interactive, non-metadata) ARE guarded; interactive never is.
        assert!(should_take_over_stdout(
            &parse(&["-p", "hi"]),
            AppMode::Print
        ));
        assert!(should_take_over_stdout(&parse(&["--json"]), AppMode::Json));
        assert!(!should_take_over_stdout(
            &parse(&["--help"]),
            AppMode::Print
        ));
        assert!(!should_take_over_stdout(&parse(&[]), AppMode::Interactive));
    }

    #[test]
    fn help_body_contains_pi_catalogue_examples_and_tools() {
        let help = render_help(&[]);
        assert!(help.contains("Environment Variables:"));
        assert!(help.contains("ANTHROPIC_API_KEY"));
        assert!(help.contains("TOGETHER_API_KEY"));
        assert!(help.contains("CYRUP_AGENT_DIR"));
        assert!(help.contains("Built-in Tool Names:"));
        assert!(help.contains("Examples:"));
        assert!(help.contains("cyrup install <source>"));
        // Extension flags inject into the body when present.
        let with_ext = render_help(&[ExtensionFlag {
            name: "plan".into(),
            value: ExtFlagValue::Bool(true),
        }]);
        assert!(with_ext.contains("Extension CLI Flags:"));
        assert!(with_ext.contains("--plan"));
    }

    #[test]
    fn config_mapping_carries_flags_and_persistence() {
        let d = dirs();
        let cli = parse(&[
            "--model",
            "faux/faux-1",
            "--system-prompt",
            "be terse",
            "--append-system-prompt",
            "cite sources",
            "--append-system-prompt",
            "stay calm",
            "--thinking",
            "low",
            "--no-tools",
            "--exclude-tools",
            "bash",
            "--no-context-files",
            "--no-skills",
            "--no-approve",
            "hello",
        ]);
        let config = cli.to_session_config(&d, AppMode::Print);
        assert_eq!(config.cwd, PathBuf::from("/work"));
        assert_eq!(config.model_pattern.as_deref(), Some("faux/faux-1"));
        assert_eq!(config.system_prompt.as_deref(), Some("be terse"));
        assert_eq!(
            config.append_system_prompt.as_deref(),
            Some("cite sources\nstay calm")
        );
        assert_eq!(config.thinking_level, Some(ModelThinkingLevel::Low));
        assert_eq!(config.no_tools, Some(NoTools::All));
        assert_eq!(config.exclude_tools, vec!["bash".to_string()]);
        assert!(config.no_context_files);
        assert!(config.no_skills);
        assert_eq!(config.trust_override, Some(false));
        assert!(!config.persist);
        assert!(matches!(config.target, SessionTarget::New));

        // Interactive persists; resume persists even in PRINT; --no-session forces ephemeral.
        assert!(cli.to_session_config(&d, AppMode::Interactive).persist);
        let resume = parse(&["--continue"]).to_session_config(&d, AppMode::Print);
        assert!(resume.persist);
        let ephemeral = parse(&["--no-session"]).to_session_config(&d, AppMode::Interactive);
        assert!(!ephemeral.persist);
    }

    #[test]
    fn to_session_config_threads_real_home_not_agent_dir() {
        // G1: the real `$HOME` (Pi `getHomeDir()`, package-manager.ts:217) must flow onto
        // `SessionConfig.home`, distinct from the agent dir, so the resources ancestor-walk dedup
        // (`~/.agents/skills`) and the trust-requiring-resource walk resolve against the real home.
        let d = dirs();
        let config = parse(&[]).to_session_config(&d, AppMode::Print);
        assert_eq!(config.home, d.home);
        assert_eq!(config.home, PathBuf::from("/home/user"));
        // The gap was `home` silently equalling the agent dir (the `SessionConfig::new` default).
        assert_ne!(config.home, config.agent_dir);
    }

    #[test]
    fn extension_flags_thread_into_config() {
        let d = dirs();
        // Explicit `-e` paths resolve to absolute vs cwd; `-ne` sets the discovery-disable flag.
        let cli = parse(&["--extension", "ext-a.ts", "-e", "/abs/ext-b", "-ne"]);
        let config = cli.to_session_config(&d, AppMode::Print);
        assert!(config.no_extensions);
        assert_eq!(
            config.extra_extension_paths,
            vec![PathBuf::from("/work/ext-a.ts"), PathBuf::from("/abs/ext-b")]
        );
        // Default: no discovery-disable, no explicit paths.
        let bare = parse(&[]).to_session_config(&d, AppMode::Print);
        assert!(!bare.no_extensions);
        assert!(bare.extra_extension_paths.is_empty());
    }

    #[test]
    fn captured_extension_flag_values_thread_into_config() {
        // Pi `extensionFlagValues: parsed.unknownFlags` (main.ts:634): the unknown `--flag[=val]`
        // tokens partitioned out before clap must reach `SessionConfig` (and thence the services), so
        // a loaded extension can read them. The bin sets `extension_flags` after clap; verify the
        // mapping from the bin's `ExtFlagValue` to the svc `ExtensionFlagValue` is faithful.
        let d = dirs();
        let (clean, flags) = partition_extension_flags(&[
            "--plan".to_string(),
            "--reviewer=alice".to_string(),
            "hi".to_string(),
        ]);
        let mut full = vec!["cyrup".to_string()];
        full.extend(clean);
        let mut cli = Cli::try_parse_from(full).expect("clap parse of the cleaned argv");
        cli.extension_flags = flags;
        let config = cli.to_session_config(&d, AppMode::Print);
        assert_eq!(
            config.extension_flag_values,
            vec![
                ("plan".to_string(), SvcExtensionFlagValue::Bool(true)),
                (
                    "reviewer".to_string(),
                    SvcExtensionFlagValue::Str("alice".to_string())
                ),
            ]
        );
        // No unknown flags ⇒ an empty threaded set (the live path carries nothing extra).
        assert!(
            parse(&["hi"])
                .to_session_config(&d, AppMode::Print)
                .extension_flag_values
                .is_empty()
        );
    }

    #[test]
    fn resource_flags_thread_into_config() {
        let d = dirs();
        let cli = parse(&[
            "--skill",
            "s1",
            "--skill",
            "s2",
            "--theme",
            "t1",
            "--prompt-template",
            "p1",
            "--no-themes",
            "--no-prompt-templates",
        ]);
        let config = cli.to_session_config(&d, AppMode::Print);
        // Relative resource paths are resolved to absolute vs the cwd (`/work`) before threading.
        assert_eq!(
            config.extra_skill_paths,
            vec![PathBuf::from("/work/s1"), PathBuf::from("/work/s2")]
        );
        assert_eq!(config.extra_theme_paths, vec![PathBuf::from("/work/t1")]);
        assert_eq!(config.extra_prompt_paths, vec![PathBuf::from("/work/p1")]);
        assert!(config.no_themes);
        assert!(config.no_prompt_templates);
    }

    #[test]
    fn scoped_model_pattern_matching() {
        assert_eq!(
            split_model_level("sonnet:high"),
            ("sonnet".to_string(), Some(ThinkingArg::High))
        );
        assert_eq!(
            split_model_level("anthropic/claude"),
            ("anthropic/claude".to_string(), None)
        );
        // A non-level suffix is preserved.
        assert_eq!(split_model_level("a:b"), ("a:b".to_string(), None));

        assert!(qualified_matches("anthropic/claude-sonnet", "anthropic/*"));
        assert!(qualified_matches("anthropic/claude-sonnet", "*sonnet*"));
        assert!(qualified_matches("openai/gpt-4o", "gpt-4o"));
        assert!(!qualified_matches("openai/gpt-4o", "anthropic/*"));
        assert!(!qualified_matches("openai/gpt-4o", "claude"));
    }

    #[test]
    fn lenient_args_feed_clap_without_a_hard_error() {
        use crate::diagnostics::{DiagnosticLevel, apply_arg_leniency};

        // The full bin pipeline: normalize → leniency → partition → clap. A bad `--mode` and a bad
        // `--thinking` must NOT make clap exit-2; they are dropped/warned by the leniency layer.
        let pipeline = |args: &[&str]| -> (Cli, Vec<crate::diagnostics::Diagnostic>) {
            let norm = normalize_short_aliases(args.iter().map(|s| s.to_string()));
            let (lenient, diags) = apply_arg_leniency(&norm);
            let (clean, ext) = partition_extension_flags(&lenient);
            let mut full = vec!["cyrup".to_string()];
            full.extend(clean);
            let mut cli = Cli::try_parse_from(full).expect("lenient argv parses under clap");
            cli.extension_flags = ext;
            (cli, diags)
        };

        // Bad --mode: silently ignored ⇒ default text mode, no diagnostics.
        let (cli, diags) = pipeline(&["--mode", "bogus", "hi"]);
        assert_eq!(cli.mode, None);
        assert_eq!(cli.positionals, vec!["hi".to_string()]);
        assert!(diags.is_empty());

        // Bad --thinking: warns + continues, no thinking set.
        let (cli, diags) = pipeline(&["--thinking", "ultra", "go"]);
        assert_eq!(cli.thinking, None);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagnosticLevel::Warning);

        // Unknown single-dash option: error diagnostic, the rest still parses.
        let (cli, diags) = pipeline(&["-x", "hello"]);
        assert_eq!(cli.positionals, vec!["hello".to_string()]);
        assert!(diags.iter().any(|d| d.level == DiagnosticLevel::Error));

        // A valid mode/thinking pair still parses normally.
        let (cli, diags) = pipeline(&["--mode", "json", "--thinking", "high"]);
        assert_eq!(cli.mode, Some(Mode::Json));
        assert_eq!(cli.thinking, Some(ThinkingArg::High));
        assert!(diags.is_empty());
    }

    #[test]
    fn model_flag_is_parsed_regardless_of_position() {
        // A `--model` placed AFTER the bare prompt must still be parsed as the model flag.
        let after = parse(&[
            "-p",
            "Reply with pong",
            "--model",
            "together/moonshotai/Kimi-K2.6",
        ]);
        assert_eq!(
            after.model.as_deref(),
            Some("together/moonshotai/Kimi-K2.6")
        );
        assert_eq!(after.positionals, vec!["Reply with pong".to_string()]);
    }
}
