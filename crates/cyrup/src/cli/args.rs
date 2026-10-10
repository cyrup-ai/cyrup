use std::path::PathBuf;

use clap::Parser;

use super::argv::ExtensionFlag;
use super::enums::{Mode, ThinkingArg, TuiMode};

/// The cyrup command line (arch-11 §3.7; Pi `cli/args.ts`).
///
/// Mode precedence (R-11-001), resolved by [`crate::cli::resolve_app_mode`]: `--acp`/`--mode acp` ▷
/// `--mode rpc` ▷ `--mode json` ▷ `--print` ▷ (no TTY) PRINT ▷ interactive TUI.
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
    /// Shorthand for `--mode acp` — serve the Agent Client Protocol on stdio (ACP-002).
    ///
    /// CYRUP-DELTA — SEAM-057. This is the one long flag cyrup keeps that pi's `parseArgs` has no
    /// arm for (`cli/args.ts` @v0.87.1), and being in `KNOWN_LONG_FLAGS` means an extension can
    /// never register `--acp` for itself. It stays because cyrup's ACP host has no pi counterpart
    /// at all (pi-acp is a separate binary) and `--acp` is the launch contract ACP clients are
    /// configured with: `docs/guide/guides/zed-acp.md` registers `"args": ["--acp"]`, and
    /// `acp_terminal_login_cmd::strip` removes it from the argv the terminal-login auth method
    /// re-launches. The other three cyrup-only aliases (`--json`, `--rpc`, `--output-format`)
    /// duplicated `--mode json`/`--mode rpc`/`--print` with nothing depending on them, and were
    /// removed so those names reach the extension-flag capture exactly as they do in pi.
    #[arg(long)]
    pub acp: bool,

    // ---- provider / model (args.ts:87-92,130) ----
    /// Provider name (e.g. `openai`, `anthropic`); combines with `--model`.
    #[arg(long = "provider", allow_hyphen_values = true)]
    pub provider: Option<String>,
    /// Model selection pattern (`provider/id[:level]`).
    #[arg(long = "model", allow_hyphen_values = true)]
    pub model: Option<String>,
    /// Runtime API key for the selected provider (defaults to env vars).
    #[arg(long = "api-key", allow_hyphen_values = true)]
    pub api_key: Option<String>,
    /// Thinking level: off, minimal, low, medium, high, xhigh, max.
    #[arg(long = "thinking", value_enum)]
    pub thinking: Option<ThinkingArg>,
    /// Comma-separated model patterns for Ctrl+P cycling (globs/fuzzy/`:level`).
    ///
    /// `None` when the flag is absent; `Some` — possibly empty — when it was supplied, which is
    /// pi's `parsed.models ?? settingsManager.getEnabledModels()` distinction (`main.ts:812-815`
    /// @f1b2e77f5). SEAM-147: empty entries are dropped by `normalize_list_flags`.
    #[arg(long = "models", value_delimiter = ',', allow_hyphen_values = true)]
    pub models: Option<Vec<String>>,

    // ---- prompt assembly (args.ts:93-97) ----
    /// Replace the assembled system prompt entirely.
    #[arg(long = "system-prompt", allow_hyphen_values = true)]
    pub system_prompt: Option<String>,
    /// Append text after the assembled system prompt (repeatable).
    #[arg(long = "append-system-prompt", allow_hyphen_values = true)]
    pub append_system_prompt: Vec<String>,

    // ---- tools (args.ts:116-129) ----
    /// Disable all tools by default (built-in and extension).
    #[arg(long = "no-tools")]
    pub no_tools: bool,
    /// Disable built-in tools by default but keep extension/custom tools enabled.
    #[arg(long = "no-builtin-tools")]
    pub no_builtin_tools: bool,
    /// Comma-separated tool allowlist, or `+name`/`-name` entries that change the default tools.
    ///
    /// SEAM-148 — pi v1.1.0 (`ddaa0a034`; `cli/args.ts:151-161`, help `:320-322` @f1b2e77f5). A list of
    /// only modifiers is applied to the default selection by the session builder; a mixed or
    /// patterned modifier list is refused in `apply_arg_leniency`. `allow_hyphen_values` lets
    /// `-t -bash` take `-bash` as the value, as pi's unconditional `args[++i]` does.
    ///
    /// `Option` keeps a SUPPLIED-but-empty list (`--tools ""`, `--tools ","`) apart from an absent
    /// flag: pi assigns the filtered `[]` (`args.ts:151-160`), and `sdk.ts:280-293` then makes it
    /// both the selection and the allowlist, so the session starts with no tools.
    #[arg(
        short = 't',
        long = "tools",
        value_delimiter = ',',
        allow_hyphen_values = true
    )]
    pub tools: Option<Vec<String>>,
    /// Comma-separated denylist of tool names to disable.
    #[arg(
        long = "exclude-tools",
        value_delimiter = ',',
        allow_hyphen_values = true
    )]
    pub exclude_tools: Vec<String>,

    // ---- resources (args.ts:149-170) ----
    /// Load an extension file or `builtin:<name>` (repeatable).
    #[arg(short = 'e', long = "extension", allow_hyphen_values = true)]
    pub extension: Vec<PathBuf>,
    /// Disable extension discovery (explicit `-e` paths still work).
    #[arg(long = "no-extensions")]
    pub no_extensions: bool,
    /// Load a skill file or directory (repeatable).
    #[arg(long = "skill", allow_hyphen_values = true)]
    pub skill: Vec<PathBuf>,
    /// Disable skills discovery and loading.
    #[arg(long = "no-skills")]
    pub no_skills: bool,
    /// Load a prompt template file or directory (repeatable).
    #[arg(long = "prompt-template", allow_hyphen_values = true)]
    pub prompt_template: Vec<PathBuf>,
    /// Disable prompt template discovery and loading.
    #[arg(long = "no-prompt-templates")]
    pub no_prompt_templates: bool,
    /// Load a theme file or directory (repeatable).
    #[arg(long = "theme", allow_hyphen_values = true)]
    pub theme: Vec<PathBuf>,
    /// The initial interactive theme for this run, a name or a `light/dark` pair (pi
    /// `--use-theme <name[/name]>`, `cli/args.ts:190-197` @v0.87.1, added v0.84.4). A one-run
    /// override that is never written to settings; ignored outside interactive mode (`main.ts:666`).
    /// pi assigns it, so a repeated flag keeps the last value.
    #[arg(long = "use-theme", value_name = "NAME", overrides_with = "use_theme")]
    pub use_theme: Option<String>,
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
    #[arg(long = "session", allow_hyphen_values = true)]
    pub session: Option<String>,
    /// Use the exact project session ID, creating it if missing.
    #[arg(long = "session-id", allow_hyphen_values = true)]
    pub session_id: Option<String>,
    /// Fork a specific session file or partial UUID into a new session.
    #[arg(long = "fork", allow_hyphen_values = true)]
    pub fork: Option<String>,
    /// Directory for session storage and lookup.
    #[arg(long = "session-dir", allow_hyphen_values = true)]
    pub session_dir: Option<PathBuf>,
    /// Don't save the session (ephemeral).
    #[arg(long = "no-session")]
    pub no_session: bool,
    /// Set the session display name.
    ///
    /// SEAM-152 — `allow_hyphen_values` here and on every other flag pi reads with an unconditional
    /// `args[++i]` (`crate::cli::UNCONDITIONAL_VALUE_FLAGS`): `--name -nc` names the session `-nc`,
    /// as in pi. The pre-clap passes leave that token alone and report a value-less flag pi's way.
    #[arg(short = 'n', long = "name", allow_hyphen_values = true)]
    pub name: Option<String>,

    // ---- standalone actions (args.ts:147,171) ----
    /// Export a session file to HTML and exit (optional output path positional).
    #[arg(long = "export", allow_hyphen_values = true)]
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
    /// Disable startup network operations (same as `CYRUP_OFFLINE=1`).
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
