use std::path::PathBuf;

use clap::Parser;

use super::argv::ExtensionFlag;
use super::enums::{Mode, OutputFormat, ThinkingArg, TuiMode};

/// The cyrup command line (arch-11 §3.7; Pi `cli/args.ts`).
///
/// Mode precedence (R-11-001), resolved by [`crate::cli::resolve_app_mode`]: `--rpc`/`--mode rpc` ▷
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
    ///
    /// A plain `bool`, deliberately: `clap::ArgAction::Version` prints `{display_name} {version}`
    /// and exits from INSIDE the parse, i.e. before `main`'s diagnostics gate, where pi does both
    /// the other way round — `main.ts:562-570` reports every parse diagnostic and exits 1 on any
    /// error-severity one, and only then `:573-576` does `if (parsed.version) { console.log(VERSION);
    /// process.exit(0); }` with a bare semver and no program name. `--help` was already ordered that
    /// way here (`main.rs`'s `if cli.help` sits after the gate); `--version` now matches. SEAM-052.
    #[arg(short = 'v', long = "version")]
    pub version: bool,
    /// Show the rich help body and exit (`-h`/`--help`). Pi prints its own hand-rolled help
    /// (args.ts:212), so clap's auto-help is disabled and [`crate::cli::render_help`] is used instead.
    #[arg(short = 'h', long = "help")]
    pub help: bool,

    // ---- mode selection (R-11-001; args.ts:78) ----
    /// Output mode: `text` (default), `json`, or `rpc`.
    #[arg(long = "mode", value_enum)]
    pub mode: Option<Mode>,
    /// One-shot PRINT mode: run to completion, print the final assistant text, exit.
    #[arg(short = 'p', long)]
    pub print: bool,
    // CYRUP-DELTA — SEAM-057. The next three flags are **cyrup-invented**: `git grep -nE
    // '"--output-format"|"--json"|"--rpc"' v0.84.1 -- packages/coding-agent/src` matches only
    // `cli/auth-command.ts:82-84` (an auth SUBCOMMAND flag) and three npm/ripgrep argv strings, and
    // pi's `parseArgs` has no such arm at either tag. Each would fall through to pi's unknown-long-
    // flag arm (`cli/args.ts:188-201`), land in `unknownFlags`, and — with no extension registering
    // it — produce `Unknown option(s): --json` + `process.exit(1)`
    // (`core/agent-session-services.ts:119-124`, `main.ts:844-848`).
    //
    // Two consequences, and the second is the one that matters: `cyrup --json` succeeds where
    // `pi --json` is a hard exit-1, and — because all three are in `KNOWN_LONG_FLAGS` /
    // `KNOWN_VALUE_LONG_FLAGS` and are therefore consumed by `partition_extension_flags` before the
    // extension-flag capture — an extension that legitimately registers a `--json` or `--rpc` flag
    // can never receive it under cyrup; the binary silently changes output mode instead. Closing
    // THAT half means deleting these three, which is a decision with users outside this crate
    // (`cyrup-it`'s own fixtures pass `--rpc`), so it is recorded here and in `render_help` below
    // rather than taken unilaterally.
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
    /// Thinking level: off, minimal, low, medium, high, xhigh, max.
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

    // ---- TUI renderer selection (args.ts:180-192 @v0.84.1) ----
    /// TUI mode: `regular` (default) or `fullscreen`. Invalid/missing values are caught by
    /// [`crate::diagnostics::apply_arg_leniency`] with pi's own two messages before clap sees them,
    /// so clap's own value error is unreachable here (the same arrangement `--thinking` uses).
    #[arg(long = "tui-mode", value_name = "MODE")]
    pub tui_mode: Option<TuiMode>,

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
    /// args.ts:188-201). Not parsed by clap — populated by [`super::argv::partition_extension_flags`] before the
    /// clap parse and set on the struct afterwards. The downstream *consumption* (feeding these to
    /// loaded extensions via `applyExtensionFlagValues`) is the outer extension tier (ledgered).
    #[arg(skip)]
    pub extension_flags: Vec<ExtensionFlag>,
}
