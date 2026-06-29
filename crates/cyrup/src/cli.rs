//! The clap argument surface and its mapping onto a [`SessionConfig`] + the runtime [`AppMode`]
//! (arch-11 §3.7/§6.1; R-11-001/024). Kept free of I/O so the mapping is unit-testable.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use cyrup_config::{AppMode, ConfigDirs};
use cyrup_session_svc::{SessionConfig, SessionTarget};

/// Output format for the non-interactive one-shot path (`--output-format`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum OutputFormat {
    /// Human-oriented final assistant text (PRINT mode).
    Text,
    /// One `AgentSessionEvent` per line (JSON mode / JSONL).
    Json,
}

/// The cyrup command line (arch-11 §3.7). A bare prompt plus mode/model/session/trust flags.
///
/// Mode precedence (R-11-001), resolved by [`resolve_app_mode`]: `--rpc` ▷ `--json` /
/// `--output-format json` ▷ `--print` ▷ (no TTY) PRINT ▷ interactive TUI.
#[derive(Parser, Debug, Default)]
#[command(name = "cyrup", version, about = "cyrup — a Rust agent harness (a port of Pi)")]
pub struct Cli {
    // ---- mode selection (R-11-001) ----
    /// One-shot PRINT mode: run to completion, print the final assistant text, exit.
    #[arg(short = 'p', long)]
    pub print: bool,
    /// One-shot output format: `text` (PRINT) or `json` (JSONL event stream).
    #[arg(long = "output-format", value_enum)]
    pub output_format: Option<OutputFormat>,
    /// Shorthand for `--output-format json` (the JSONL `AgentSessionEvent` stream).
    #[arg(long)]
    pub json: bool,
    /// RPC mode: the persistent stdin/stdout line protocol for cross-process embedding.
    #[arg(long)]
    pub rpc: bool,

    // ---- model ----
    /// Model selection pattern (`provider/id[:level]`).
    #[arg(short = 'm', long)]
    pub model: Option<String>,

    // ---- prompt assembly ----
    /// Replace the assembled system prompt entirely.
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,
    /// Append text after the assembled system prompt.
    #[arg(long = "append-system-prompt")]
    pub append_system_prompt: Option<String>,

    // ---- resources ----
    /// Do not load `CYRUP.md`/`AGENTS.md` context files.
    #[arg(long = "no-context-files")]
    pub no_context_files: bool,
    /// Do not load skills.
    #[arg(long = "no-skills")]
    pub no_skills: bool,

    // ---- trust (arch-07 / R-11-029) ----
    /// Trust the project for this run (`--approve`); enables trust-requiring resources.
    #[arg(short = 'a', long = "approve")]
    pub approve: bool,
    /// Refuse project trust for this run (`--no-approve`).
    #[arg(long = "no-approve")]
    pub no_approve: bool,
    /// Run as if invoked from `<dir>` (changes the working directory used for resolution).
    #[arg(short = 'C', long = "cwd")]
    pub cwd: Option<PathBuf>,

    // ---- session ----
    /// Continue the most recent session for this cwd.
    #[arg(short = 'c', long = "continue")]
    pub r#continue: bool,
    /// Resume a specific session file by path.
    #[arg(long = "resume")]
    pub resume: Option<PathBuf>,

    // ---- diagnostics ----
    /// Increase log verbosity (stderr only; never pollutes the protocol stream).
    #[arg(short = 'v', long)]
    pub verbose: bool,

    /// The prompt: bare message words and `@file` references, merged with piped stdin (R-11-006/025).
    #[arg(value_name = "PROMPT")]
    pub positionals: Vec<String>,
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

    /// Which session the run targets (resume ▷ continue ▷ new).
    pub fn session_target(&self) -> SessionTarget {
        match &self.resume {
            Some(path) => SessionTarget::Resume(path.clone()),
            None if self.r#continue => SessionTarget::Continue,
            None => SessionTarget::New,
        }
    }

    /// Map the CLI + resolved directories + runtime mode onto a [`SessionConfig`] (arch-11 §3.7).
    ///
    /// Persistence (R-11-008): one-shot PRINT/JSON default to an ephemeral in-memory session unless a
    /// session is explicitly resumed/continued; interactive always persists.
    pub fn to_session_config(&self, dirs: &ConfigDirs, mode: AppMode) -> SessionConfig {
        let mut config = SessionConfig::new(dirs.cwd.clone(), dirs.agent_dir.clone());
        config.session_dir = Some(dirs.session_dir.clone());
        config.app_mode = mode;
        config.model_pattern = self.model.clone();
        config.trust_override = self.trust_override();
        config.no_context_files = self.no_context_files;
        config.no_skills = self.no_skills;
        config.system_prompt = self.system_prompt.clone();
        config.append_system_prompt = self.append_system_prompt.clone();
        config.target = self.session_target();
        let explicit_session =
            matches!(config.target, SessionTarget::Resume(_) | SessionTarget::Continue);
        config.persist = explicit_session || mode == AppMode::Interactive;
        config
    }
}

/// Resolve the runtime mode (R-11-001 / arch-11 §6.1). Explicit mode flags win; otherwise a
/// non-TTY stdin or stdout forces PRINT, and a full TTY pair selects the interactive front-end.
pub fn resolve_app_mode(cli: &Cli, stdin_tty: bool, stdout_tty: bool) -> AppMode {
    if cli.rpc {
        return AppMode::Rpc;
    }
    if cli.json || cli.output_format == Some(OutputFormat::Json) {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        let mut full = vec!["cyrup"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full).expect("parse")
    }

    #[test]
    fn mode_flags_take_precedence_over_tty() {
        // --rpc wins over everything.
        assert_eq!(resolve_app_mode(&parse(&["--rpc"]), true, true), AppMode::Rpc);
        // --json / --output-format json => JSON.
        assert_eq!(resolve_app_mode(&parse(&["--json"]), true, true), AppMode::Json);
        assert_eq!(
            resolve_app_mode(&parse(&["--output-format", "json"]), true, true),
            AppMode::Json
        );
        // -p => PRINT even with a full TTY.
        assert_eq!(resolve_app_mode(&parse(&["-p"]), true, true), AppMode::Print);
    }

    #[test]
    fn tty_probing_selects_interactive_or_print() {
        let cli = parse(&[]);
        assert_eq!(resolve_app_mode(&cli, true, true), AppMode::Interactive);
        // A redirected stdin or stdout forces one-shot PRINT.
        assert_eq!(resolve_app_mode(&cli, false, true), AppMode::Print);
        assert_eq!(resolve_app_mode(&cli, true, false), AppMode::Print);
    }

    #[test]
    fn trust_override_maps_approve_flags() {
        assert_eq!(parse(&["--approve"]).trust_override(), Some(true));
        assert_eq!(parse(&["--no-approve"]).trust_override(), Some(false));
        assert_eq!(parse(&[]).trust_override(), None);
        // Approve wins when both are set.
        assert_eq!(parse(&["--approve", "--no-approve"]).trust_override(), Some(true));
    }

    #[test]
    fn session_target_resume_continue_new() {
        assert!(matches!(parse(&["--resume", "/tmp/s.jsonl"]).session_target(), SessionTarget::Resume(_)));
        assert!(matches!(parse(&["-c"]).session_target(), SessionTarget::Continue));
        assert!(matches!(parse(&[]).session_target(), SessionTarget::New));
        // Resume wins over continue.
        assert!(matches!(
            parse(&["-c", "--resume", "/tmp/s.jsonl"]).session_target(),
            SessionTarget::Resume(_)
        ));
    }

    #[test]
    fn config_mapping_carries_flags_and_persistence() {
        let dirs = ConfigDirs {
            agent_dir: "/agent".into(),
            session_dir: "/agent/sessions".into(),
            package_dir: "/agent/packages".into(),
            cwd: "/work".into(),
        };

        let cli = parse(&[
            "-m", "faux/faux-1",
            "--system-prompt", "be terse",
            "--append-system-prompt", "cite sources",
            "--no-context-files",
            "--no-skills",
            "--no-approve",
            "hello",
        ]);
        let config = cli.to_session_config(&dirs, AppMode::Print);
        assert_eq!(config.cwd, std::path::PathBuf::from("/work"));
        assert_eq!(config.agent_dir, std::path::PathBuf::from("/agent"));
        assert_eq!(config.session_dir, Some(std::path::PathBuf::from("/agent/sessions")));
        assert_eq!(config.model_pattern.as_deref(), Some("faux/faux-1"));
        assert_eq!(config.system_prompt.as_deref(), Some("be terse"));
        assert_eq!(config.append_system_prompt.as_deref(), Some("cite sources"));
        assert!(config.no_context_files);
        assert!(config.no_skills);
        assert_eq!(config.trust_override, Some(false));
        assert_eq!(config.app_mode, AppMode::Print);
        // One-shot PRINT with no explicit session => ephemeral.
        assert!(!config.persist);
        assert!(matches!(config.target, SessionTarget::New));

        // Interactive persists; resume persists even in PRINT.
        assert!(cli.to_session_config(&dirs, AppMode::Interactive).persist);
        let resume = parse(&["--resume", "/tmp/s.jsonl"]).to_session_config(&dirs, AppMode::Print);
        assert!(resume.persist);
    }

    #[test]
    fn model_flag_is_parsed_regardless_of_position() {
        // Regression: a `-m` placed AFTER the bare prompt must be parsed as the model flag,
        // not swallowed by the prompt positional (the old `trailing_var_arg` bug silently fell
        // back to faux).
        let after = parse(&["-p", "Reply with pong", "-m", "together/moonshotai/Kimi-K2.6"]);
        assert_eq!(after.model.as_deref(), Some("together/moonshotai/Kimi-K2.6"));
        assert_eq!(after.positionals, vec!["Reply with pong".to_string()]);

        let before = parse(&["-p", "-m", "together/moonshotai/Kimi-K2.6", "Reply with pong"]);
        assert_eq!(before.model.as_deref(), Some("together/moonshotai/Kimi-K2.6"));
        assert_eq!(before.positionals, vec!["Reply with pong".to_string()]);
    }
}
