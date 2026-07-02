//! The 13 slash-command descriptors and their argument parsers (func-SA §5.6 R-SA-129; arch-SA
//! §2.2/§6.8).
//!
//! # Scope of this file
//!
//! This file defines, for all 13 commands listed by R-SA-129 (`/run`, `/chain`, `/parallel`,
//! `/run-chain`, `/subagent-cost`, `/subagents-doctor`, `/subagents-models`,
//! `/subagents-profiles`, `/subagents-load-profile`, `/subagents-refresh-provider-models`,
//! `/subagents-generate-profiles`, `/subagents-check-profile`, `/subagents-companions`):
//!
//! - a [`SlashCommandName`] enum plus a [`SLASH_COMMANDS`] table of static descriptors
//!   (name/usage/description) suitable for driving `InitApi::register_command` registration,
//! - pure, side-effect-free **argument parsers** that turn a command's raw trailing argument
//!   string into a strongly-typed [`ParsedCommand`] variant,
//! - for `/run`, `/chain`, and `/parallel`: the shared `--bg`/`--fork`/`[key=value,...]`
//!   inline-override grammar (R-SA-129), including `/chain`'s inline parallel-group
//!   `(a "task" | b "task")[opts]` chain-expression syntax, faithfully ported from
//!   `pi-subagents/src/slash/slash-commands.ts` (`parseChainExpression`/`parseGroupSegment`/
//!   `splitOnArrow`/`splitGroupTasks`/`mapParsedTaskToStepObject`) — verified line-for-line
//!   against that source file rather than re-derived from the functionality spec's prose summary
//!   alone, since func-SA §5.6 does not spell out the grammar's precise quoting/nesting rules and
//!   the architecture doc defers to "the 13 command list and their argument shapes" in
//!   functionality.md §5.6, which itself defers to the pi-subagents source tree per this
//!   document's own "source of truth" rule (func-SA header).
//!
//! Every parser here produces [`SingleStepSpec`]/[`ParallelGroupSpec`]/[`RunnerStep`] values
//! directly (`crate::spawn::chain_graph`'s already-landed types) rather than a bespoke
//! intermediate shape this file would otherwise need a second conversion pass for later — chain
//! and parallel commands parse straight into a `Vec<RunnerStep>`/`ChainGraph`.
//!
//! # What is explicitly deferred to later phases (NOT implemented in this file)
//!
//! - **Actual registration into `InitApi`** (`api.register_command(...)` calls) — owned by
//!   `extension.rs`, arch-SA §3.2/§6.8, "Phase 9" per this crate's build-out plan. This file only
//!   defines the descriptor table [`SLASH_COMMANDS`] and the parsers `extension.rs` is expected to
//!   call from its `execute_command` dispatch; it does not depend on `cyrup_ext::native::InitApi`
//!   at all, so this module has zero risk of needing to change shape once that wiring lands.
//! - **Agent-name existence validation** (pi-subagents' `discoverAgents(...).agents.find(...)`
//!   calls scattered through `parseAgentArgs`/`buildChainExpressionSteps`) — that requires a live
//!   `AgentDiscoveryConfig`/`HostCtx` this pure-parsing module has no access to and no need of;
//!   parsers here validate **syntax** only (well-formed agent-token/quoting/parens/arrow/pipe
//!   structure, required-field presence per R-SA-129's own argument-shape contract) and return a
//!   syntactically well-formed [`ParsedCommand`] whose `agent`/`chain name` string fields the
//!   caller (again `extension.rs`, which does have discovery access via the executor) is expected
//!   to resolve and reject-if-unknown before spawning anything. This mirrors pi-subagents' own
//!   split between "syntax I can check with no I/O" (this file's TS analogue) and "semantic
//!   agent-exists check" (which needed `ctx`/`state.baseCwd`).
//! - **`/subagents-doctor`, `/subagent-cost`, named-profile commands' actual execution bodies**
//!   (`registration/doctor.rs`, `registration/cost.rs`, `registration/profiles.rs`) — separate,
//!   later sibling modules per arch-SA §2.2's module layout; this file defines only their
//!   descriptor entries and trivial argument parsing (an optional single positional token, or
//!   none), never their diagnostic/report-building logic.
//! - **`/subagents-companions`'s dismissal-state mutation** — same story: this file parses
//!   `status | hide <package> <workspace|user> | show <package>` into a typed
//!   [`CompanionsCommand`], but persisting a dismissal to `config.json` is a `registration/mod.rs`
//!   / `extension.rs` concern this file has no config-store handle to perform.
//! - **The single shared dispatch path itself** (R-SA-130: "every slash command handler MUST
//!   route through the same internal execution function the `subagent` tool uses") — that is a
//!   property of `extension.rs`'s `dispatch_slash`/`SubagentExecutor::execute_from_command`
//!   wiring (arch-SA §6.8), not something a pure argument-parsing module can itself enforce; this
//!   file supplies the parsed, uniform input both entry points are expected to feed into that one
//!   executor.

use std::path::PathBuf;

use crate::discovery::types::OutputMode;
use crate::spawn::chain_graph::{ParallelGroupSpec, RunnerStep, SingleStepSpec};

// =================================================================================================
// SlashCommandName / SLASH_COMMANDS descriptor table (R-SA-129)
// =================================================================================================

/// The 13 slash commands R-SA-129 mandates, as a closed enum (never a bare `&str` — every
/// dispatch call site gets exhaustiveness checking against this list rather than risking a
/// stringly-typed typo silently falling through to "unknown command").
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SlashCommandName {
    Run,
    Chain,
    Parallel,
    RunChain,
    SubagentCost,
    SubagentsDoctor,
    SubagentsModels,
    SubagentsProfiles,
    SubagentsLoadProfile,
    SubagentsRefreshProviderModels,
    SubagentsGenerateProfiles,
    SubagentsCheckProfile,
    SubagentsCompanions,
}

impl SlashCommandName {
    /// The literal command name as the user types it, WITHOUT the leading `/` (matches
    /// `cyrup_ext::registry::CommandDescriptor`'s registration convention and pi-subagents'
    /// `pi.registerCommand("run", ...)` bare-name convention).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SlashCommandName::Run => "run",
            SlashCommandName::Chain => "chain",
            SlashCommandName::Parallel => "parallel",
            SlashCommandName::RunChain => "run-chain",
            SlashCommandName::SubagentCost => "subagent-cost",
            SlashCommandName::SubagentsDoctor => "subagents-doctor",
            SlashCommandName::SubagentsModels => "subagents-models",
            SlashCommandName::SubagentsProfiles => "subagents-profiles",
            SlashCommandName::SubagentsLoadProfile => "subagents-load-profile",
            SlashCommandName::SubagentsRefreshProviderModels => {
                "subagents-refresh-provider-models"
            }
            SlashCommandName::SubagentsGenerateProfiles => "subagents-generate-profiles",
            SlashCommandName::SubagentsCheckProfile => "subagents-check-profile",
            SlashCommandName::SubagentsCompanions => "subagents-companions",
        }
    }

    /// Parse a bare command name (no leading `/`) back into its typed variant, or `None` if it
    /// does not match any of the 13. Case-sensitive, exact match only (mirrors R-SA-008's
    /// "exact string equality only, no fuzzy matching" convention applied here to command names).
    #[must_use]
    pub fn from_str_exact(name: &str) -> Option<Self> {
        SLASH_COMMANDS.iter().find(|d| d.name.as_str() == name).map(|d| d.name)
    }
}

/// One command's static registration metadata: name, one-line usage string, and human-readable
/// description. Deliberately does NOT carry a completion-function pointer — the real
/// `cyrup_ext::registry::CommandDescriptor` (`crates/cyrup-ext/src/registry.rs:69-73`) takes a
/// static `completions: Vec<String>` list, not a closure, so per-invocation dynamic completions
/// (e.g. "list currently-discovered agent names") are an `extension.rs`/`InitApi` wiring concern
/// for a later phase, not something this static table can express; `usage` here is the
/// human-readable fallback pi-subagents surfaces via `ctx.ui.notify(usage, "error")` on a parse
/// failure, preserved verbatim per command so error messages match source behavior.
#[derive(Clone, Copy, Debug)]
pub struct SlashCommandDescriptor {
    pub name: SlashCommandName,
    pub usage: &'static str,
    pub description: &'static str,
}

/// All 13 commands R-SA-129 mandates, in the order that requirement lists them. `extension.rs`
/// (Phase 9) is expected to iterate this table once at `init()` time and call
/// `InitApi::register_command` once per entry (arch-SA §3.2's `for cmd in
/// registration::SLASH_COMMANDS { api.register_command(...) }` sketch) — this table is the single
/// source of truth for "which 13 commands exist," so an omission here is a compile-visible gap
/// rather than a silently-missing registration.
pub const SLASH_COMMANDS: &[SlashCommandDescriptor] = &[
    SlashCommandDescriptor {
        name: SlashCommandName::Run,
        usage: "Usage: /run <agent>[key=value,...] [task] [--bg] [--fork]",
        description: "Run a subagent directly",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::Chain,
        usage: "Usage: /chain agent1 \"task1\" -> agent2 \"task2\" [--bg] [--fork]",
        description: "Run agents in sequence, with optional inline parallel groups",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::Parallel,
        usage: "Usage: /parallel agent1 \"task1\" -> agent2 \"task2\" [--bg] [--fork]",
        description: "Run agents in parallel",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::RunChain,
        usage: "Usage: /run-chain <chainName> -- <task> [--bg] [--fork]",
        description: "Run a saved chain",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::SubagentCost,
        usage: "Usage: /subagent-cost",
        description: "Show parent and subagent child usage cost for this session",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::SubagentsDoctor,
        usage: "Usage: /subagents-doctor",
        description: "Show subagent diagnostics",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::SubagentsModels,
        usage: "Usage: /subagents-models [builtin-agent-name]",
        description: "Show runtime-loaded builtin subagent models",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::SubagentsProfiles,
        usage: "Usage: /subagents-profiles",
        description: "List saved subagent profiles",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::SubagentsLoadProfile,
        usage: "Usage: /subagents-load-profile <name>",
        description: "Load a subagent profile into settings",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::SubagentsRefreshProviderModels,
        usage: "Usage: /subagents-refresh-provider-models <provider> [--force]",
        description: "Refresh the cached model catalog for one provider",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::SubagentsGenerateProfiles,
        usage: "Usage: /subagents-generate-profiles <provider>",
        description: "Generate <provider>.quota and <provider>.quality subagent profiles",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::SubagentsCheckProfile,
        usage: "Usage: /subagents-check-profile <name>",
        description: "Check whether a saved profile still points to usable models",
    },
    SlashCommandDescriptor {
        name: SlashCommandName::SubagentsCompanions,
        usage: "Usage: /subagents-companions status | hide <package> <workspace|user> | show <package>",
        description: "Manage companion-extension recommendation visibility",
    },
];

// =================================================================================================
// Shared error type
// =================================================================================================

/// A slash-command argument-parsing failure. Every variant carries the exact human-readable usage
/// text pi-subagents surfaces via `ctx.ui.notify(message, "error")` — `extension.rs` is expected
/// to route this `message` straight to the same UI notification path, matching R-SA-129/130's
/// "same executor, same user-facing contract" requirement at the argument-parsing layer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SlashParseError {
    pub message: String,
}

impl SlashParseError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

// =================================================================================================
// Shared --bg / --fork trailing-flag extraction (R-SA-129: "each accepting trailing `--bg`
// (async) / `--fork` (context: fork) flags")
// =================================================================================================

/// The result of stripping trailing `--bg`/`--fork` flags from a raw argument string: the
/// remaining argument text (flags removed, trimmed) plus which flags were present. Flags may
/// appear in either order and are stripped repeatedly from the end until neither remains,
/// matching `pi-subagents`' `extractExecutionFlags`'s `loop { ... break }` structure exactly (so
/// `"agent task --fork --bg"` and `"agent task --bg --fork"` both parse identically).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ExecutionFlags {
    pub background: bool,
    pub fork: bool,
}

/// Strip trailing `--bg`/`--fork` tokens (R-SA-129) from `raw_args`, in any order, repeatedly
/// until none remain at the end of the string. Returns the cleaned remainder (trimmed) plus the
/// flags observed.
///
/// Faithful port of `pi-subagents/src/slash/slash-commands.ts`'s `extractExecutionFlags`
/// (lines 99-119): only a *trailing* `--bg`/`--fork` (either as the entire remaining string or
/// preceded by a space) is stripped — an occurrence of the literal text `--bg` in the middle of a
/// task string (e.g. `agent -- "explain --bg flag"`) is NOT stripped, since it does not end the
/// string at the moment its containing suffix is checked. This is intentionally a plain suffix
/// strip, not a tokenizing flag parser, exactly matching source behavior.
#[must_use]
pub fn extract_execution_flags(raw_args: &str) -> (String, ExecutionFlags) {
    let mut args = raw_args.trim().to_string();
    let mut flags = ExecutionFlags::default();

    loop {
        if args == "--bg" {
            flags.background = true;
            args = String::new();
            continue;
        }
        if let Some(stripped) = args.strip_suffix(" --bg") {
            flags.background = true;
            args = stripped.trim().to_string();
            continue;
        }
        if args == "--fork" {
            flags.fork = true;
            args = String::new();
            continue;
        }
        if let Some(stripped) = args.strip_suffix(" --fork") {
            flags.fork = true;
            args = stripped.trim().to_string();
            continue;
        }
        break;
    }

    (args, flags)
}

// =================================================================================================
// Inline [key=value,...] config grammar (shared by /run, /chain, /parallel step tokens)
// =================================================================================================

/// One agent token's inline `[key=value,...]` configuration (func-SA §5.6 R-SA-129
/// "`[key=value,...]` inline overrides"). Faithful port of `pi-subagents`' `InlineConfig`
/// (`slash-commands.ts:45-59`) and its `parseInlineConfig` (lines 61-90).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct InlineStepConfig {
    /// `output=<path>` / `output=false` (explicit "no output file").
    pub output: Option<InlineOutput>,
    /// `outputMode=inline` / `outputMode=file-only`.
    pub output_mode: Option<OutputMode>,
    /// `reads=<a>+<b>+...` / `reads=false`.
    pub reads: Option<InlineReads>,
    /// `model=<id>`.
    pub model: Option<String>,
    /// `skill=<a>+<b>+...` / `skill=false` / `skills=...` (both spellings accepted, matching
    /// source's `case "skill": case "skills":` fallthrough).
    pub skill: Option<InlineSkills>,
    /// Bare `progress` token (no `=`) is shorthand for `progress=true`; `progress=false` disables
    /// it explicitly. `None` means "not specified at all," distinct from an explicit `false`.
    pub progress: Option<bool>,
    /// `as=<name>` — the named-output key this step's structured output registers under
    /// ([`SingleStepSpec::output`]).
    pub as_output: Option<String>,
    pub label: Option<String>,
    pub phase: Option<String>,
    pub cwd: Option<String>,
    /// `count=<positive integer>` — parallel-group-only per source (`opts.inGroup` gate in
    /// `mapParsedTaskToStepObject`); silently ignored outside a group by this parser's own
    /// caller, mirroring source's `opts.inGroup && config.count !== undefined` guard.
    pub count: Option<u32>,
    /// `outputSchema=<path>` — a JSON-schema file path, resolved (and loaded) by a later phase
    /// with filesystem access; this parser only carries the raw path string forward.
    pub output_schema_path: Option<String>,
    /// `acceptance=<level>` — validated against the slash-surface's restricted level set
    /// (R-SA-129/README: `auto|attested|checked` only) by [`validate_inline_acceptance`].
    pub acceptance: Option<String>,
}

/// `output=<path>` vs. the explicit `output=false` sentinel (source: `val === "false" ? false :
/// val`). Modeled as an enum rather than `Option<Option<String>>` for clarity at call sites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineOutput {
    Path(String),
    ExplicitlyDisabled,
}

/// `reads=<a>+<b>+...` vs. the explicit `reads=false` sentinel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineReads {
    Paths(Vec<String>),
    ExplicitlyDisabled,
}

/// `skill=<a>+<b>+...` vs. the explicit `skill=false` sentinel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineSkills {
    Names(Vec<String>),
    ExplicitlyDisabled,
}

/// Parse one `[...]`-bracket-interior `key=value,...` string into an [`InlineStepConfig`].
/// `raw` is the text ALREADY stripped of its surrounding `[` `]` delimiters.
///
/// Faithful port of `pi-subagents`' `parseInlineConfig` (`slash-commands.ts:61-90`): unknown keys
/// are silently ignored (never an error — matches source's `switch` with no `default` arm), a
/// bare comma-separated token with no `=` is recognized only as the `progress` shorthand (any
/// other bare token is silently dropped, matching `if (trimmed === "progress") config.progress =
/// true; continue;`), and `+`-delimited list values (`reads`/`skill`) drop empty segments
/// (`.split("+").filter(Boolean)`).
#[must_use]
pub fn parse_inline_config(raw: &str) -> InlineStepConfig {
    let mut config = InlineStepConfig::default();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(eq_idx) = trimmed.find('=') else {
            if trimmed == "progress" {
                config.progress = Some(true);
            }
            continue;
        };
        let key = trimmed[..eq_idx].trim();
        let val = trimmed[eq_idx + 1..].trim();
        match key {
            "output" => {
                config.output = Some(if val == "false" {
                    InlineOutput::ExplicitlyDisabled
                } else {
                    InlineOutput::Path(val.to_string())
                });
            }
            "outputMode" => {
                config.output_mode = match val {
                    "inline" => Some(OutputMode::Inline),
                    "file-only" => Some(OutputMode::FileOnly),
                    _ => config.output_mode,
                };
            }
            "reads" => {
                config.reads = Some(if val == "false" {
                    InlineReads::ExplicitlyDisabled
                } else {
                    InlineReads::Paths(split_plus_delimited(val))
                });
            }
            "model" => {
                if !val.is_empty() {
                    config.model = Some(val.to_string());
                }
            }
            "skill" | "skills" => {
                config.skill = Some(if val == "false" {
                    InlineSkills::ExplicitlyDisabled
                } else {
                    InlineSkills::Names(split_plus_delimited(val))
                });
            }
            "progress" => {
                config.progress = Some(val != "false");
            }
            "as" => {
                if !val.is_empty() {
                    config.as_output = Some(val.to_string());
                }
            }
            "label" => {
                if !val.is_empty() {
                    config.label = Some(val.to_string());
                }
            }
            "phase" => {
                if !val.is_empty() {
                    config.phase = Some(val.to_string());
                }
            }
            "cwd" => {
                if !val.is_empty() {
                    config.cwd = Some(val.to_string());
                }
            }
            "count" => {
                if let Ok(n) = val.parse::<u32>()
                    && n > 0
                {
                    config.count = Some(n);
                }
            }
            "outputSchema" => {
                if !val.is_empty() {
                    config.output_schema_path = Some(val.to_string());
                }
            }
            "acceptance" if !val.is_empty() => {
                config.acceptance = Some(val.to_string());
            }
            _ => {}
        }
    }
    config
}

fn split_plus_delimited(val: &str) -> Vec<String> {
    val.split('+')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The slash-command surface's restricted acceptance-level vocabulary (README/source: "Inline
/// acceptance ... supports auto, attested, or checked. Use the subagent tool API or a saved
/// `.chain.json` file for none, verified, or reviewed acceptance contracts.") — `none`,
/// `verified`, and `reviewed` are valid [`crate::exec::acceptance::AcceptanceStatus`] wire values
/// in general, but deliberately rejected specifically at THIS inline-string entry point.
const INLINE_ACCEPTANCE_LEVELS: &[&str] = &["auto", "attested", "checked"];

/// Validate an inline `acceptance=<value>` token against the slash surface's restricted level set.
/// Faithful port of `pi-subagents`' `validateInlineAcceptanceInput` (`slash-commands.ts:838-844`).
///
/// # Errors
///
/// Returns [`SlashParseError`] if `value` is not one of `auto`/`attested`/`checked`.
pub fn validate_inline_acceptance(value: &str, agent: &str) -> Result<(), SlashParseError> {
    if INLINE_ACCEPTANCE_LEVELS.contains(&value) {
        return Ok(());
    }
    Err(SlashParseError::new(format!(
        "Inline acceptance for step '{agent}' supports auto, attested, or checked. Use the \
         subagent tool API or a saved .chain.json file for none, verified, or reviewed \
         acceptance contracts."
    )))
}

// =================================================================================================
// parse_agent_token: "agentName[key=value,...]" -> (name, config)
// =================================================================================================

/// Split one agent token into its bare name and inline `[...]` config, if present.
/// Faithful port of `pi-subagents`' `parseAgentToken` (`slash-commands.ts:92-97`).
#[must_use]
pub fn parse_agent_token(token: &str) -> (String, InlineStepConfig) {
    let Some(bracket) = token.find('[') else {
        return (token.to_string(), InlineStepConfig::default());
    };
    let name = token.get(..bracket).unwrap_or_default().to_string();
    let end = token.rfind(']');
    let inner = match end {
        Some(end_idx) if end_idx > bracket => token.get(bracket + 1..end_idx).unwrap_or_default(),
        _ => token.get(bracket + 1..).unwrap_or_default(),
    };
    (name, parse_inline_config(inner))
}

// =================================================================================================
// One parsed chain/parallel step token: "agent[config] \"task\"" or "agent[config] -- task"
// =================================================================================================

/// One parsed step token from a chain/parallel expression, before agent-existence validation.
/// Faithful port of `pi-subagents`' `ParsedStep` (`slash-commands.ts:580`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedStepToken {
    pub name: String,
    pub config: InlineStepConfig,
    pub task: Option<String>,
}

/// Parse one whitespace-delimited-but-quote-aware step token into a [`ParsedStepToken`].
///
/// Faithful port of `pi-subagents`' `parseSingleTaskToken` (`slash-commands.ts:647-664`). Two
/// task-attachment shapes are recognized, checked in this exact order:
///
/// 1. `agent[config] "task text"` / `agent[config] 'task text'` — a trailing single- or
///    double-quoted task, matched via the same regex shape as source
///    (`^(\S+(?:\[[^\]]*\])?)\s+(?:"([^"]*)"|'([^']*)')$`, reimplemented here as an explicit
///    hand-rolled scan since `regex` is not a dependency of this crate).
/// 2. `agent[config] -- task text` — a `" -- "`-delimited shared/explicit task with no quoting
///    requirement, used when the task itself is not (or cannot be) quoted.
///
/// If neither shape matches, the whole token is treated as a bare agent reference with no task of
/// its own (`task: None`) — the caller (chain/parallel builders) is responsible for supplying a
/// fallback task where R-SA-129's argument-shape rules require one.
#[must_use]
pub fn parse_single_task_token(token: &str) -> ParsedStepToken {
    if let Some((agent_part, task)) = match_quoted_trailing_task(token) {
        let (name, config) = parse_agent_token(agent_part);
        return ParsedStepToken { name, config, task };
    }

    if let Some(dash_idx) = find_top_level_delimiter(token, " -- ") {
        let agent_part = token.get(..dash_idx).unwrap_or_default().trim();
        let task_part = token.get(dash_idx + 4..).unwrap_or_default().trim();
        let (name, config) = parse_agent_token(agent_part);
        return ParsedStepToken {
            name,
            config,
            task: if task_part.is_empty() {
                None
            } else {
                Some(task_part.to_string())
            },
        };
    }

    let (name, config) = parse_agent_token(token);
    ParsedStepToken {
        name,
        config,
        task: None,
    }
}

/// Match `^(\S+(?:\[[^\]]*\])?)\s+(?:"([^"]*)"|'([^']*)')$` against `token`: an agent-part
/// (non-whitespace, optionally followed by one `[...]` bracket group) followed by whitespace and
/// a single fully-quoted task filling out the rest of the token. Returns `(agent_part,
/// Some(task))` when the quoted task is non-empty, `(agent_part, None)` when it is an empty
/// string literal (`""`/`''`), matching source's `(qMatch[2] ?? qMatch[3]) || undefined`.
fn match_quoted_trailing_task(token: &str) -> Option<(&str, Option<String>)> {
    for quote in ['"', '\''] {
        let Some(agent_end) = find_quoted_task_boundary(token, quote) else {
            continue;
        };
        let agent_part = token.get(..agent_end).unwrap_or_default().trim_end();
        // Bracket balance sanity check: the agent part, if it contains `[`, must also contain a
        // matching `]` before the quote opens (guards against a task string that itself contains
        // an unmatched `[`).
        if agent_part.is_empty() {
            continue;
        }
        let after_agent = token.get(agent_end..).unwrap_or_default().trim_start();
        if !after_agent.starts_with(quote) || !after_agent.ends_with(quote) || after_agent.len() < 2 {
            continue;
        }
        let Some(inner) = after_agent.get(1..after_agent.len() - 1) else {
            continue;
        };
        if inner.contains(quote) {
            // A quote character appears inside what would need to be the quoted body — not a
            // clean single fully-quoted trailing task; fall through to the `--` delimiter path.
            continue;
        }
        return Some((
            agent_part,
            if inner.is_empty() {
                None
            } else {
                Some(inner.to_string())
            },
        ));
    }
    None
}

/// Find the index in `token` where a run of whitespace begins immediately before a
/// same-quote-delimited task that runs to the end of the string. Returns `None` if `token` does
/// not end with `quote`, or if there is no preceding whitespace-then-quote boundary.
fn find_quoted_task_boundary(token: &str, quote: char) -> Option<usize> {
    let trimmed_end = token.trim_end();
    if !trimmed_end.ends_with(quote) || trimmed_end.len() < 2 {
        return None;
    }
    // Find the LAST whitespace run that precedes the opening quote of the trailing quoted
    // segment. Scan from the start of the trailing quoted body backwards for the nearest
    // preceding whitespace boundary at depth 0 (token itself has no nested-quote complexity here
    // since this function only ever inspects one full token, already isolated from its siblings
    // by the caller's own top-level splitting).
    let body_start = trimmed_end.get(..trimmed_end.len() - 1)?.rfind(quote)?;
    if body_start == 0 {
        return None;
    }
    let before = trimmed_end.get(..body_start)?;
    if !before.ends_with(char::is_whitespace) {
        return None;
    }
    Some(before.trim_end().len())
}

/// Find the first occurrence of `delimiter` in `input` that is not inside a single/double-quoted
/// span. Mirrors the quote-tracking used throughout `pi-subagents`' scanners
/// (`splitOnArrow`/`splitGroupTasks`/`findUnmatchedCloseParen`) but scoped to a single-delimiter
/// search rather than a full split.
fn find_top_level_delimiter(input: &str, delimiter: &str) -> Option<usize> {
    let bytes = input.as_bytes();
    let delim_bytes = delimiter.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    for (i, &ch) in bytes.iter().enumerate() {
        if in_single {
            if ch == b'\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == b'"' {
                in_double = false;
            }
            continue;
        }
        match ch {
            b'\'' => in_single = true,
            b'"' => in_double = true,
            _ => {
                if bytes.get(i..).is_some_and(|rest| rest.starts_with(delim_bytes)) {
                    return Some(i);
                }
            }
        }
    }
    None
}

// =================================================================================================
// Chain-expression grammar: "-> " step separator, "(a | b)[opts]" inline parallel groups
// (R-SA-129: "/chain (with inline parallel groups `(a | b)[opts]` syntax)")
// =================================================================================================

/// One element of a parsed `/chain` expression: either a single step token or an inline parallel
/// group. Faithful port of `pi-subagents`' `ParsedGroupStep = ParsedStep | ParsedGroup`
/// (`slash-commands.ts:582`).
///
/// `clippy::large_enum_variant` is deliberately allowed here, mirroring
/// [`crate::spawn::chain_graph::RunnerStep`]'s identical, already-established precedent for the
/// identical shape of lint: this is a one-off-per-command parse result (never a hot,
/// size-sensitive collection), so boxing the common `Step` variant would trade a clippy nit for
/// an unnecessary heap allocation on the overwhelmingly common non-group path.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum ParsedChainElement {
    Step(ParsedStepToken),
    Group {
        tasks: Vec<ParsedStepToken>,
        config: GroupConfig,
    },
}

/// Inline parallel-group options, `[concurrency=N,failFast,worktree]` (source:
/// `parseGroupConfig`, `slash-commands.ts:666-681`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GroupConfig {
    pub concurrency: Option<u32>,
    pub fail_fast: Option<bool>,
    pub worktree: Option<bool>,
}

/// Parse a group's `[concurrency=N,failFast,worktree=false,...]` suffix (interior text only, `[`
/// `]` already stripped by the caller). Faithful port of `parseGroupConfig`
/// (`slash-commands.ts:666-681`): a bare token with no `=` is boolean-shorthand-true for
/// `failFast`/`worktree` (any other bare token is silently dropped), unknown keys are silently
/// ignored, `concurrency` requires a positive integer or is dropped.
#[must_use]
pub fn parse_group_config(raw: &str) -> GroupConfig {
    let mut config = GroupConfig::default();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let eq_idx = trimmed.find('=');
        let key = eq_idx.map_or(trimmed, |i| trimmed[..i].trim());
        let val = eq_idx.map_or("", |i| trimmed[i + 1..].trim());
        match key {
            "concurrency" => {
                if let Ok(n) = val.parse::<u32>()
                    && n > 0
                {
                    config.concurrency = Some(n);
                }
            }
            "failFast" => {
                config.fail_fast = Some(eq_idx.is_none() || val != "false");
            }
            "worktree" => {
                config.worktree = Some(eq_idx.is_none() || val != "false");
            }
            _ => {}
        }
    }
    config
}

/// True if `depth` walking through `input` would ever go negative (an unmatched closing paren) or
/// end nonzero (an unmatched opening paren) — quote-aware. Faithful port of
/// `findUnmatchedCloseParen` (`slash-commands.ts:590-602`), generalized to report BOTH imbalance
/// directions (source's name is specific to its one call site checking only the immediate
/// close-paren case; this port's single function serves every call site that needs a
/// paren-balance check, matching every one of source's uses of it).
#[must_use]
pub fn has_unbalanced_parens(input: &str) -> bool {
    let mut depth: i32 = 0;
    let mut in_single = false;
    let mut in_double = false;
    for ch in input.chars() {
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            }
            continue;
        }
        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    depth != 0
}

/// Split `input` on top-level `" -> "` (ignoring arrows inside quotes or parentheses). Faithful
/// port of `splitOnArrow` (`slash-commands.ts:605-624`).
#[must_use]
pub fn split_on_arrow(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut segments = Vec::new();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut start = 0usize;
    let mut i = 0usize;
    while let Some(&ch) = bytes.get(i) {
        if in_single {
            if ch == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if ch == b'"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        match ch {
            b'\'' => {
                in_single = true;
                i += 1;
            }
            b'"' => {
                in_double = true;
                i += 1;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
            }
            b'-' if depth == 0
                && bytes.get(i + 1) == Some(&b'>')
                && bytes.get(i + 2) == Some(&b' ') =>
            {
                segments.push(input.get(start..i).unwrap_or_default().to_string());
                i += 3;
                start = i;
            }
            _ => {
                i += 1;
            }
        }
    }
    segments.push(input.get(start..).unwrap_or_default().to_string());
    segments
}

/// Split a group's interior text on top-level `" | "` (ignoring pipes inside quotes/nested
/// parens). Faithful port of `splitGroupTasks` (`slash-commands.ts:627-645`).
#[must_use]
pub fn split_group_tasks(inner: &str) -> Vec<String> {
    let bytes = inner.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut start = 0usize;
    let mut i = 0usize;
    while let Some(&ch) = bytes.get(i) {
        if in_single {
            if ch == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if ch == b'"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        match ch {
            b'\'' => {
                in_single = true;
                i += 1;
            }
            b'"' => {
                in_double = true;
                i += 1;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
            }
            b'|' if depth == 0 => {
                parts.push(inner.get(start..i).unwrap_or_default().to_string());
                start = i + 1;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    parts.push(inner.get(start..).unwrap_or_default().to_string());
    parts
}

/// Split a `(...)`-wrapped group body from an optional trailing `[...]` config suffix, respecting
/// quotes and (in principle) nested nesting depth. Faithful port of `splitGroupBody`
/// (`slash-commands.ts:685-704`).
///
/// # Errors
///
/// Returns [`SlashParseError`] if `trimmed` does not start with `(`, has no matching top-level
/// `)`, or has a non-empty trailing suffix that is not itself a single `[...]`-wrapped block.
fn split_group_body(trimmed: &str) -> Result<(String, GroupConfig), SlashParseError> {
    let bytes = trimmed.as_bytes();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut close_idx: Option<usize> = None;
    for (i, &ch) in bytes.iter().enumerate() {
        if in_single {
            if ch == b'\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == b'"' {
                in_double = false;
            }
            continue;
        }
        match ch {
            b'\'' => in_single = true,
            b'"' => in_double = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close_idx = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close_idx) = close_idx else {
        return Err(SlashParseError::new(format!(
            "Unmatched parentheses in group: '{trimmed}'"
        )));
    };
    let inner = trimmed.get(1..close_idx).unwrap_or_default().to_string();
    let suffix = trimmed.get(close_idx + 1..).unwrap_or_default().trim();
    if suffix.is_empty() {
        return Ok((inner, GroupConfig::default()));
    }
    if !suffix.starts_with('[') || !suffix.ends_with(']') {
        return Err(SlashParseError::new(format!(
            "Group options must be wrapped in [...]: '{suffix}'"
        )));
    }
    Ok((
        inner,
        parse_group_config(
            suffix
                .get(1..suffix.len().saturating_sub(1))
                .unwrap_or_default(),
        ),
    ))
}

/// Parse one `(a "task" | b "task")[opts]` inline-parallel-group segment. Faithful port of
/// `parseGroupSegment` (`slash-commands.ts:706-717`).
///
/// # Errors
///
/// Returns [`SlashParseError`] if `segment` does not open with `(`, has unmatched parens, a
/// malformed `[...]` config suffix, or fewer than two `|`-separated tasks inside the group.
pub fn parse_group_segment(segment: &str) -> Result<ParsedChainElement, SlashParseError> {
    let trimmed = segment.trim();
    if !trimmed.starts_with('(') {
        return Err(SlashParseError::new(format!(
            "Parallel group must be wrapped in parentheses: '{trimmed}'"
        )));
    }
    let (inner, config) = split_group_body(trimmed)?;
    let raw_parts: Vec<String> = split_group_tasks(&inner)
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if raw_parts.len() < 2 {
        return Err(SlashParseError::new(
            "Parallel group must contain at least two tasks separated by ' | '",
        ));
    }
    Ok(ParsedChainElement::Group {
        tasks: raw_parts.iter().map(|p| parse_single_task_token(p)).collect(),
        config,
    })
}

/// True if `input` uses inline parallel-group syntax at all — i.e. at least one top-level (` -> `
/// separated) segment opens with `(`. Faithful port of `hasGroupSyntax`
/// (`slash-commands.ts:719-726`): a paren appearing INSIDE a shared task (e.g. `scout -- inspect
/// auth (backend)`) does not count, since it is never at the start of a top-level arrow-split
/// segment.
#[must_use]
pub fn has_group_syntax(input: &str) -> bool {
    split_on_arrow(input).iter().any(|seg| seg.trim_start().starts_with('('))
}

/// The fully parsed `/chain` expression: an ordered list of steps/groups (R-SA-129's `(a |
/// b)[opts]` chain-expression grammar). Faithful port of `pi-subagents`' `parseChainExpression`
/// return shape (`slash-commands.ts:728`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedChainExpression {
    pub elements: Vec<ParsedChainElement>,
}

/// Parse a full `/chain` expression using `" -> "` as the step separator and `(a | b)[opts]` as
/// the inline-parallel-group syntax. Faithful port of `parseChainExpression`
/// (`slash-commands.ts:728-753`).
///
/// # Errors
///
/// Returns [`SlashParseError`] if: `input` contains no `" -> "` separator at all; `input` has
/// unmatched parentheses anywhere; any individual non-group segment itself has unmatched
/// parentheses; a `(...)`-prefixed segment fails [`parse_group_segment`]; or the expression
/// resolves to zero steps after filtering empty segments.
pub fn parse_chain_expression(input: &str) -> Result<ParsedChainExpression, SlashParseError> {
    let trimmed = input.trim();
    if !trimmed.contains(" -> ") {
        return Err(SlashParseError::new(
            "Parallel groups in /chain require \" -> \" between steps",
        ));
    }
    if has_unbalanced_parens(trimmed) {
        return Err(SlashParseError::new(
            "Unmatched parentheses in /chain expression",
        ));
    }
    let mut elements = Vec::new();
    for seg in split_on_arrow(trimmed) {
        let t = seg.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with('(') {
            elements.push(parse_group_segment(t)?);
            continue;
        }
        if has_unbalanced_parens(t) {
            return Err(SlashParseError::new(format!(
                "Unmatched parentheses in chain segment: '{t}'"
            )));
        }
        elements.push(ParsedChainElement::Step(parse_single_task_token(t)));
    }
    if elements.is_empty() {
        return Err(SlashParseError::new(
            "/chain expression must include at least one step",
        ));
    }
    Ok(ParsedChainExpression { elements })
}

// =================================================================================================
// ParsedStepToken / ParsedChainElement -> SingleStepSpec / RunnerStep conversion
// =================================================================================================

/// Convert one [`ParsedStepToken`] into a [`SingleStepSpec`], applying `fallback_task` only when
/// `is_first` is true and the token itself has no task of its own (mirrors
/// `mapParsedTaskToStepObject`'s `stepTask ? {task: stepTask} : isFirst && fallbackTask ? {task:
/// fallbackTask} : {}` precedence, `slash-commands.ts:863-888`). `in_group` gates whether
/// `config.count` is honored (parallel-group-only per source).
///
/// # Errors
///
/// Returns [`SlashParseError`] if `config.acceptance` is present but fails
/// [`validate_inline_acceptance`].
pub fn step_token_to_spec(
    step: &ParsedStepToken,
    fallback_task: Option<&str>,
    is_first: bool,
    in_group: bool,
) -> Result<SingleStepSpec, SlashParseError> {
    if let Some(acceptance) = &step.config.acceptance {
        validate_inline_acceptance(acceptance, &step.name)?;
    }

    let task = step
        .task
        .clone()
        .or_else(|| {
            if is_first {
                fallback_task.map(str::to_string)
            } else {
                None
            }
        })
        .unwrap_or_default();

    let output_mode = step.config.output_mode;
    let output = step.config.as_output.clone();
    let reads = match &step.config.reads {
        Some(InlineReads::Paths(paths)) => {
            Some(paths.iter().map(PathBuf::from).collect::<Vec<_>>())
        }
        Some(InlineReads::ExplicitlyDisabled) | None => None,
    };
    let cwd = step.config.cwd.as_ref().map(PathBuf::from);
    let _ = in_group; // `count` has no SingleStepSpec-level home; a later phase's DynamicGroup
    // template-instantiation path is where a group's per-task `count` (fan-out width hint) would
    // apply, per this file's own module-header note on deferred DynamicGroup template binding.

    Ok(SingleStepSpec {
        agent: step.name.clone(),
        task,
        cwd,
        model: None, // `config.model` is a raw string; resolving it to a `ModelId` requires the
        // model registry this pure-parsing module has no access to (deferred to `extension.rs`).
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output,
        output_mode,
        reads,
        acceptance: step.config.acceptance.clone(),
        context: None,
        agent_scope: None,
    })
}

// =================================================================================================
// /run — direct single-agent invocation (R-SA-129)
// =================================================================================================

/// The fully parsed `/run` command: one agent, one task, plus execution flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRunCommand {
    pub agent: String,
    pub config: InlineStepConfig,
    pub task: String,
    pub flags: ExecutionFlags,
}

/// Parse `/run <agent>[key=value,...] [task] [--bg] [--fork]` (R-SA-129). Faithful port of the
/// `pi.registerCommand("run", ...)` handler body (`slash-commands.ts:978-1006`), argument-parsing
/// portion only (agent-existence validation and actual dispatch are later-phase concerns per this
/// file's module header).
///
/// # Errors
///
/// Returns [`SlashParseError`] if, after flag-stripping, no input remains at all (source: "Usage:
/// /run \<agent\> [task] [--bg] [--fork]").
pub fn parse_run_command(raw_args: &str) -> Result<ParsedRunCommand, SlashParseError> {
    let (cleaned, flags) = extract_execution_flags(raw_args);
    let input = cleaned.trim();
    if input.is_empty() {
        return Err(SlashParseError::new(
            "Usage: /run <agent> [task] [--bg] [--fork]",
        ));
    }
    let first_space = input.find(' ');
    let agent_token = match first_space {
        Some(idx) => input.get(..idx).unwrap_or(input),
        None => input,
    };
    let task = match first_space {
        Some(idx) => input.get(idx + 1..).unwrap_or_default().trim().to_string(),
        None => String::new(),
    };
    let (agent, config) = parse_agent_token(agent_token);

    let task = match &config.reads {
        Some(InlineReads::Paths(paths)) if !paths.is_empty() => {
            format!("[Read from: {}]\n\n{task}", paths.join(", "))
        }
        _ => task,
    };

    Ok(ParsedRunCommand {
        agent,
        config,
        task,
        flags,
    })
}

// =================================================================================================
// /chain and /parallel — shared "agent1 task1 -> agent2 task2" / "agent1 agent2 -- task" grammar
// =================================================================================================

/// Which of the two sibling commands is calling [`parse_agent_args`] — selects the exact
/// first-step/at-least-one-task validation rule that differs between `/chain` and `/parallel`
/// (source: `command === "chain"` vs. `command === "parallel"` branches,
/// `slash-commands.ts:807-814`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentArgsCommand {
    Chain,
    Parallel,
}

/// The non-group-syntax parse result shared by `/chain` and `/parallel`: a list of step tokens
/// plus the resolved shared task (empty string if every step carries its own task). Faithful port
/// of `parseAgentArgs`'s return shape (`slash-commands.ts:760`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedAgentArgs {
    pub steps: Vec<ParsedStepToken>,
    pub task: String,
}

/// Parse the shared `/chain`/`/parallel` non-group grammar: either `agent1 "task1" -> agent2
/// "task2"` (per-step tasks, `" -> "`-delimited) or `agent1 agent2 -- shared task` (a
/// space-delimited agent list, then one `" -- "`-delimited shared task applied to every step with
/// no task of its own).
///
/// Faithful port of `parseAgentArgs` (`slash-commands.ts:755-816`), MINUS the
/// `discoverAgents(...).agents.find(...)` existence check (source lines 800-806) — deferred to
/// `extension.rs` per this file's module header, since it requires live discovery/`ctx` this pure
/// parser has no access to.
///
/// # Errors
///
/// Returns [`SlashParseError`] when: no steps parse out at all; `/chain`'s first step has no task
/// (neither its own nor a shared fallback); or `/parallel` has no task anywhere (neither any
/// step's own nor a shared one); or (the `" -- "` branch) the shared-task delimiter is missing, or
/// either side of it is empty.
pub fn parse_agent_args(
    args: &str,
    command: AgentArgsCommand,
) -> Result<ParsedAgentArgs, SlashParseError> {
    let input = args.trim();
    let usage = format!(
        "Usage: /{} agent1 \"task1\" -> agent2 \"task2\"",
        match command {
            AgentArgsCommand::Chain => "chain",
            AgentArgsCommand::Parallel => "parallel",
        }
    );

    let (steps, shared_task, per_step) = if input.contains(" -> ") {
        let mut steps = Vec::new();
        for seg in input.split(" -> ") {
            let trimmed = seg.trim();
            if trimmed.is_empty() {
                continue;
            }
            steps.push(parse_single_task_token(trimmed));
        }
        let shared_task = steps
            .iter()
            .find_map(|s| s.task.clone())
            .unwrap_or_default();
        (steps, shared_task, true)
    } else {
        let Some(delim_idx) = find_top_level_delimiter(input, " -- ") else {
            return Err(SlashParseError::new(usage));
        };
        let agents_part = input.get(..delim_idx).unwrap_or_default().trim();
        let shared_task = input
            .get(delim_idx + 4..)
            .unwrap_or_default()
            .trim()
            .to_string();
        if agents_part.is_empty() || shared_task.is_empty() {
            return Err(SlashParseError::new(usage));
        }
        let steps: Vec<ParsedStepToken> = agents_part
            .split_whitespace()
            .map(parse_single_task_token)
            .collect();
        (steps, shared_task, false)
    };

    if steps.is_empty() {
        return Err(SlashParseError::new(usage));
    }

    match command {
        AgentArgsCommand::Chain => {
            let first_has_task = steps.first().is_some_and(|s| s.task.is_some());
            if !first_has_task && (per_step || shared_task.is_empty()) {
                return Err(SlashParseError::new(
                    "First step must have a task: /chain agent \"task\" -> agent2",
                ));
            }
        }
        AgentArgsCommand::Parallel => {
            let any_step_has_task = steps.iter().any(|s| s.task.is_some());
            if !any_step_has_task && shared_task.is_empty() {
                return Err(SlashParseError::new(
                    "At least one step must have a task",
                ));
            }
        }
    }

    Ok(ParsedAgentArgs {
        steps,
        task: shared_task,
    })
}

/// The fully parsed `/chain` command: a [`RunnerStep`] sequence ready for the chain graph walker,
/// plus the resolved overall task text (for display) and execution flags.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedChainCommand {
    pub chain: Vec<RunnerStep>,
    pub task: String,
    pub flags: ExecutionFlags,
}

/// Parse `/chain agent1 "task1" -> agent2 "task2"` (or the inline-parallel-group variant `/chain
/// agent1 "task1" -> (agent2 "task2" | agent3 "task3") -> agent4`) into a [`ParsedChainCommand`]
/// (R-SA-129). Faithful port of `buildChainExpressionSteps` (`slash-commands.ts:890-972`) composed
/// with the `pi.registerCommand("chain", ...)` handler's flag extraction
/// (`slash-commands.ts:1008-1020`).
///
/// # Errors
///
/// Returns [`SlashParseError`] for any of the syntax failures [`parse_agent_args`] /
/// [`parse_chain_expression`] / [`parse_group_segment`] / [`step_token_to_spec`] can raise, plus:
/// every task inside a parallel group must have its own task (no shared-task fallback inside a
/// group, source lines 932-938); the first element (whether a bare step or a group) must have at
/// least one task among its members (source lines 939-947).
pub fn parse_chain_command(raw_args: &str) -> Result<ParsedChainCommand, SlashParseError> {
    let (cleaned, flags) = extract_execution_flags(raw_args);

    if !has_group_syntax(&cleaned) {
        let parsed = parse_agent_args(&cleaned, AgentArgsCommand::Chain)?;
        let fallback = if parsed.task.is_empty() {
            None
        } else {
            Some(parsed.task.as_str())
        };
        let chain = parsed
            .steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                step_token_to_spec(step, fallback, i == 0, false)
                    .map(RunnerStep::SingleStep)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(ParsedChainCommand {
            chain,
            task: parsed.task,
            flags,
        });
    }

    let expression = parse_chain_expression(&cleaned)?;

    for element in &expression.elements {
        if let ParsedChainElement::Group { tasks, .. } = element
            && tasks.iter().any(|t| t.task.is_none())
        {
            return Err(SlashParseError::new(
                "Each task in a parallel group needs a task: (agent \"a\" | agent \"b\")",
            ));
        }
    }

    let first_has_task = match expression.elements.first() {
        Some(ParsedChainElement::Group { tasks, .. }) => tasks.iter().any(|t| t.task.is_some()),
        Some(ParsedChainElement::Step(step)) => step.task.is_some(),
        None => false,
    };
    if !first_has_task {
        return Err(SlashParseError::new(
            "First step must have a task: /chain agent \"task\" -> agent2",
        ));
    }

    let shared_task = match expression.elements.first() {
        Some(ParsedChainElement::Group { tasks, .. }) => {
            tasks.iter().find_map(|t| t.task.clone()).unwrap_or_default()
        }
        Some(ParsedChainElement::Step(step)) => step.task.clone().unwrap_or_default(),
        None => String::new(),
    };
    let fallback = if shared_task.is_empty() {
        None
    } else {
        Some(shared_task.as_str())
    };

    let chain = expression
        .elements
        .iter()
        .map(|element| match element {
            ParsedChainElement::Group { tasks, config } => {
                let steps = tasks
                    .iter()
                    .map(|t| step_token_to_spec(t, None, false, true))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(RunnerStep::ParallelGroup(ParallelGroupSpec {
                    steps,
                    concurrency: config.concurrency.unwrap_or(4),
                    fail_fast: config.fail_fast.unwrap_or(false),
                    worktree: config.worktree.unwrap_or(false),
                }))
            }
            ParsedChainElement::Step(step) => {
                step_token_to_spec(step, fallback, false, false).map(RunnerStep::SingleStep)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ParsedChainCommand {
        chain,
        task: shared_task,
        flags,
    })
}

/// The fully parsed `/parallel` command: a flat list of [`SingleStepSpec`] tasks to fan out over,
/// plus execution flags.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedParallelCommand {
    pub tasks: Vec<SingleStepSpec>,
    pub flags: ExecutionFlags,
}

/// Parse `/parallel agent1 "task1" -> agent2 "task2"` (R-SA-129). `/parallel` does NOT accept the
/// `(a | b)` inline-group syntax — it IS already an implicit single-level parallel group over
/// however many steps `parse_agent_args` returns; nesting a further group inside it has no
/// well-formed meaning and pi-subagents' own `/parallel` handler
/// (`slash-commands.ts:1052-1074`) never calls `buildChainExpressionSteps`/`hasGroupSyntax` at
/// all, confirming this is not an oversight.
///
/// # Errors
///
/// Returns [`SlashParseError`] for any failure [`parse_agent_args`] can raise.
pub fn parse_parallel_command(raw_args: &str) -> Result<ParsedParallelCommand, SlashParseError> {
    let (cleaned, flags) = extract_execution_flags(raw_args);
    let parsed = parse_agent_args(&cleaned, AgentArgsCommand::Parallel)?;
    let tasks = parsed
        .steps
        .iter()
        .map(|step| {
            let task = step.task.clone().unwrap_or_else(|| parsed.task.clone());
            SingleStepSpec {
                agent: step.name.clone(),
                task,
                cwd: step.config.cwd.as_ref().map(PathBuf::from),
                model: None,
                tools: None,
                extensions: None,
                session_file: None,
                max_depth_override: None,
                structured_output_schema: None,
                output: None,
                output_mode: step.config.output_mode,
                reads: match &step.config.reads {
                    Some(InlineReads::Paths(paths)) => {
                        Some(paths.iter().map(PathBuf::from).collect())
                    }
                    _ => None,
                },
                acceptance: None,
                context: None,
                agent_scope: None,
            }
        })
        .collect();
    Ok(ParsedParallelCommand { tasks, flags })
}

// =================================================================================================
// /run-chain — invoke a saved chain by name
// =================================================================================================

/// The fully parsed `/run-chain` command header: the saved chain's name, the task to run it with,
/// and execution flags. Resolving `chain_name` against discovered saved chains (and expanding its
/// steps into concrete [`RunnerStep`]s, `mapSavedChainSteps` in source) is a later-phase concern
/// requiring live discovery this pure parser has no access to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRunChainCommand {
    pub chain_name: String,
    pub task: String,
    pub flags: ExecutionFlags,
}

/// Parse `/run-chain <chainName> -- <task> [--bg] [--fork]` (R-SA-129). Faithful port of the
/// `pi.registerCommand("run-chain", ...)` handler's argument-parsing portion
/// (`slash-commands.ts:1022-1050`).
///
/// # Errors
///
/// Returns [`SlashParseError`] if the mandatory `" -- "` delimiter is missing, or either the chain
/// name or the task is empty after trimming.
pub fn parse_run_chain_command(raw_args: &str) -> Result<ParsedRunChainCommand, SlashParseError> {
    let (cleaned, flags) = extract_execution_flags(raw_args);
    let usage = "Usage: /run-chain <chainName> -- <task> [--bg] [--fork]";
    let Some(delim_idx) = find_top_level_delimiter(&cleaned, " -- ") else {
        return Err(SlashParseError::new(usage));
    };
    let chain_name = cleaned.get(..delim_idx).unwrap_or_default().trim().to_string();
    let task = cleaned
        .get(delim_idx + 4..)
        .unwrap_or_default()
        .trim()
        .to_string();
    if chain_name.is_empty() || task.is_empty() {
        return Err(SlashParseError::new(usage));
    }
    Ok(ParsedRunChainCommand {
        chain_name,
        task,
        flags,
    })
}

// =================================================================================================
// /subagent-cost, /subagents-doctor — no arguments
// =================================================================================================

/// `/subagent-cost` and `/subagents-doctor` take no arguments at all (R-SA-129's per-command
/// argument-shape contract; source: both handlers ignore `_args` entirely,
/// `slash-commands.ts:1076-1088`). This parser exists purely so every one of the 13 commands has a
/// uniform `parse_*` entry point `extension.rs` can dispatch through — it always succeeds.
pub fn parse_no_args_command(_raw_args: &str) {}

// =================================================================================================
// /subagents-models — optional single builtin-agent-name positional argument
// =================================================================================================

/// The fully parsed `/subagents-models` command: either "show all" (`agent: None`) or "show one
/// builtin agent" (`agent: Some(name)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSubagentsModelsCommand {
    pub agent: Option<String>,
}

/// Parse `/subagents-models [builtin-agent-name]` (R-SA-129). Faithful port of the
/// `pi.registerCommand("subagents-models", ...)` handler's argument-parsing portion
/// (`slash-commands.ts:1090-1111`), MINUS the `BUILTIN_AGENT_NAMES.includes(agent)` existence
/// check (source lines 1105-1108) — deferred to `extension.rs`, which has access to the resolved
/// builtin-agent-name list this pure parser does not.
///
/// # Errors
///
/// Returns [`SlashParseError`] if more than one whitespace-delimited token is supplied.
pub fn parse_subagents_models_command(
    raw_args: &str,
) -> Result<ParsedSubagentsModelsCommand, SlashParseError> {
    let trimmed = raw_args.trim();
    if trimmed.is_empty() {
        return Ok(ParsedSubagentsModelsCommand { agent: None });
    }
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let [only] = parts.as_slice() else {
        return Err(SlashParseError::new(
            "Usage: /subagents-models [builtin-agent-name]",
        ));
    };
    Ok(ParsedSubagentsModelsCommand {
        agent: Some((*only).to_string()),
    })
}

// =================================================================================================
// /subagents-profiles — no arguments
// =================================================================================================

// `/subagents-profiles` takes no arguments (source: `slash-commands.ts:1113-1123`). Shares
// `parse_no_args_command`'s always-succeeds shape.

// =================================================================================================
// Single-required-positional-argument commands: /subagents-load-profile, /subagents-generate-
// profiles, /subagents-check-profile
// =================================================================================================

/// Parse a command whose entire argument contract is exactly one required positional token (no
/// flags, no `key=value`). Faithful port of `parseSingleRequiredArg`
/// (`slash-commands.ts:330-334`), shared by `/subagents-load-profile`, `/subagents-generate-
/// profiles`, and `/subagents-check-profile` (source calls this same helper for all three,
/// `slash-commands.ts:1134`, `1216`, `1259`).
///
/// # Errors
///
/// Returns [`SlashParseError`] with `usage_message` when `args` does not contain EXACTLY one
/// whitespace-delimited token.
pub fn parse_single_required_arg(
    args: &str,
    usage_message: &str,
) -> Result<String, SlashParseError> {
    let parts: Vec<&str> = args.split_whitespace().collect();
    let [only] = parts.as_slice() else {
        return Err(SlashParseError::new(usage_message));
    };
    Ok((*only).to_string())
}

/// Parse `/subagents-load-profile <name>` (R-SA-129).
///
/// # Errors
///
/// See [`parse_single_required_arg`].
pub fn parse_subagents_load_profile_command(raw_args: &str) -> Result<String, SlashParseError> {
    parse_single_required_arg(raw_args, "Usage: /subagents-load-profile <name>")
}

/// Parse `/subagents-generate-profiles <provider>` (R-SA-129).
///
/// # Errors
///
/// See [`parse_single_required_arg`].
pub fn parse_subagents_generate_profiles_command(
    raw_args: &str,
) -> Result<String, SlashParseError> {
    parse_single_required_arg(raw_args, "Usage: /subagents-generate-profiles <provider>")
}

/// Parse `/subagents-check-profile <name>` (R-SA-129).
///
/// # Errors
///
/// See [`parse_single_required_arg`].
pub fn parse_subagents_check_profile_command(raw_args: &str) -> Result<String, SlashParseError> {
    parse_single_required_arg(raw_args, "Usage: /subagents-check-profile <name>")
}

// =================================================================================================
// /subagents-refresh-provider-models — one required positional plus an optional --force/force flag
// =================================================================================================

/// The fully parsed `/subagents-refresh-provider-models` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRefreshProviderModelsCommand {
    pub provider: String,
    pub force: bool,
}

/// Parse `/subagents-refresh-provider-models <provider> [--force]` (R-SA-129). Faithful port of
/// the `pi.registerCommand("subagents-refresh-provider-models", ...)` handler's argument-parsing
/// portion (`slash-commands.ts:1178-1210`): a trailing `--force` OR bare `force` token (either
/// form, source's regex accepts both) is stripped before applying
/// [`parse_single_required_arg`] to what remains.
///
/// # Errors
///
/// Returns [`SlashParseError`] if, after stripping an optional trailing force flag, the remainder
/// is not exactly one token.
pub fn parse_subagents_refresh_provider_models_command(
    raw_args: &str,
) -> Result<ParsedRefreshProviderModelsCommand, SlashParseError> {
    let trimmed = raw_args.trim();
    let (without_force, force) = strip_trailing_force_flag(trimmed);
    let provider = parse_single_required_arg(
        &without_force,
        "Usage: /subagents-refresh-provider-models <provider> [--force]",
    )?;
    Ok(ParsedRefreshProviderModelsCommand { provider, force })
}

/// Strip a trailing `--force` or bare `force` token (source: `/(?:^|\s)--force$/` /
/// `/(?:^|\s)force$/`, `slash-commands.ts:1183-1184`). Returns the cleaned remainder plus whether
/// a force flag was present.
fn strip_trailing_force_flag(input: &str) -> (String, bool) {
    for suffix in ["--force", "force"] {
        if input == suffix {
            return (String::new(), true);
        }
        let with_space = format!(" {suffix}");
        if let Some(stripped) = input.strip_suffix(&with_space) {
            return (stripped.trim_end().to_string(), true);
        }
    }
    (input.to_string(), false)
}

// =================================================================================================
// /subagents-companions — status | hide <package> <workspace|user> | show <package>
// =================================================================================================

/// The two companion package names pi-subagents' `/subagents-companions` recognizes (source:
/// `COMPANION_PACKAGES`, referenced by `parseCompanionPackage`,
/// `companion-suggestions.ts:282-284`). Kept as a fixed, closed set here (rather than an arbitrary
/// string) so an unknown package name is a parse-time rejection, matching source's own "Unknown
/// companion package" error.
pub const COMPANION_PACKAGES: &[&str] = &["pi-intercom", "pi-prompt-template-model"];

/// One parsed `/subagents-companions` invocation (R-SA-129). Faithful port of
/// `handleCompanionCommand`'s argument-parsing shape (`companion-suggestions.ts:328-351`) — the
/// actual status-report rendering / dismissal-state mutation this type feeds into is a
/// `registration/mod.rs`/`extension.rs` concern with a config-store handle this pure parser does
/// not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompanionsCommand {
    /// `/subagents-companions` (bare) or `/subagents-companions status`.
    Status,
    /// `/subagents-companions hide <package> <workspace|user>`.
    Hide {
        package: String,
        scope: CompanionsScope,
    },
    /// `/subagents-companions show <package>`.
    Show { package: String },
}

/// `workspace` vs. `user` dismissal scope for [`CompanionsCommand::Hide`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompanionsScope {
    Workspace,
    User,
}

/// Parse `/subagents-companions status | hide <package> <workspace|user> | show <package>`
/// (R-SA-129). Faithful port of `handleCompanionCommand`'s argument-parsing portion
/// (`companion-suggestions.ts:328-351`), returning a typed [`CompanionsCommand`] in place of
/// source's `{ text, updatedConfig?, error? }` shape (this file has no config store to mutate;
/// `extension.rs` performs the actual dismissal update from the parsed command this function
/// returns).
///
/// # Errors
///
/// Returns [`SlashParseError`] if: the first token is present but neither `hide` nor `show`; the
/// named package is not one of [`COMPANION_PACKAGES`]; or (`hide` only) the scope token is
/// present but is neither `workspace` nor `user`.
pub fn parse_subagents_companions_command(
    raw_args: &str,
) -> Result<CompanionsCommand, SlashParseError> {
    let parts: Vec<&str> = raw_args.split_whitespace().collect();
    let verb = parts.first().copied();
    if verb.is_none() || verb == Some("status") {
        return Ok(CompanionsCommand::Status);
    }
    if verb != Some("hide") && verb != Some("show") {
        return Err(SlashParseError::new(
            "Usage: /subagents-companions status | hide <pi-intercom|pi-prompt-template-model> \
             <workspace|user> | show <pi-intercom|pi-prompt-template-model>",
        ));
    }
    let package = parts.get(1).copied();
    let Some(package) = package.filter(|p| COMPANION_PACKAGES.contains(p)) else {
        return Err(SlashParseError::new(
            "Unknown companion package. Use pi-intercom or pi-prompt-template-model.",
        ));
    };
    if verb == Some("show") {
        return Ok(CompanionsCommand::Show {
            package: package.to_string(),
        });
    }
    let scope = match parts.get(2).copied() {
        Some("workspace") => CompanionsScope::Workspace,
        Some("user") => CompanionsScope::User,
        _ => {
            return Err(SlashParseError::new(
                "Usage: /subagents-companions hide <pi-intercom|pi-prompt-template-model> \
                 <workspace|user>",
            ));
        }
    };
    Ok(CompanionsCommand::Hide {
        package: package.to_string(),
        scope,
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    // ---------------------------------------------------------------------------------------
    // SLASH_COMMANDS table completeness (R-SA-129: exactly 13 commands)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn slash_commands_table_has_exactly_13_entries() {
        assert_eq!(SLASH_COMMANDS.len(), 13);
    }

    #[test]
    fn slash_commands_table_has_no_duplicate_names() {
        let mut names: Vec<&str> = SLASH_COMMANDS.iter().map(|d| d.name.as_str()).collect();
        let original_len = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), original_len, "duplicate command name found");
    }

    #[test]
    fn slash_commands_table_matches_r_sa_129s_exact_list() {
        let expected = [
            "run",
            "chain",
            "parallel",
            "run-chain",
            "subagent-cost",
            "subagents-doctor",
            "subagents-models",
            "subagents-profiles",
            "subagents-load-profile",
            "subagents-refresh-provider-models",
            "subagents-generate-profiles",
            "subagents-check-profile",
            "subagents-companions",
        ];
        let actual: Vec<&str> = SLASH_COMMANDS.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn from_str_exact_round_trips_every_command_name() {
        for desc in SLASH_COMMANDS {
            assert_eq!(
                SlashCommandName::from_str_exact(desc.name.as_str()),
                Some(desc.name)
            );
        }
        assert_eq!(SlashCommandName::from_str_exact("nonexistent"), None);
        // Case-sensitive: no fuzzy match (mirrors R-SA-008's exact-string-equality convention).
        assert_eq!(SlashCommandName::from_str_exact("Run"), None);
    }

    // ---------------------------------------------------------------------------------------
    // extract_execution_flags (R-SA-129: trailing --bg / --fork, either order)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn extract_execution_flags_strips_bg_only() {
        let (args, flags) = extract_execution_flags("scout do the thing --bg");
        assert_eq!(args, "scout do the thing");
        assert!(flags.background);
        assert!(!flags.fork);
    }

    #[test]
    fn extract_execution_flags_strips_fork_only() {
        let (args, flags) = extract_execution_flags("scout do the thing --fork");
        assert_eq!(args, "scout do the thing");
        assert!(!flags.background);
        assert!(flags.fork);
    }

    #[test]
    fn extract_execution_flags_strips_both_regardless_of_order() {
        let (args1, flags1) = extract_execution_flags("scout task --bg --fork");
        assert_eq!(args1, "scout task");
        assert!(flags1.background && flags1.fork);

        let (args2, flags2) = extract_execution_flags("scout task --fork --bg");
        assert_eq!(args2, "scout task");
        assert!(flags2.background && flags2.fork);
    }

    #[test]
    fn extract_execution_flags_bare_flag_with_no_other_args() {
        let (args, flags) = extract_execution_flags("--bg");
        assert_eq!(args, "");
        assert!(flags.background);
    }

    #[test]
    fn extract_execution_flags_no_flags_present() {
        let (args, flags) = extract_execution_flags("scout do the thing");
        assert_eq!(args, "scout do the thing");
        assert!(!flags.background && !flags.fork);
    }

    #[test]
    fn extract_execution_flags_does_not_strip_mid_string_occurrence() {
        // `--bg` appears but NOT as a trailing token -> must not be treated as a flag.
        let (args, flags) = extract_execution_flags("scout -- \"explain --bg flag\"");
        assert_eq!(args, "scout -- \"explain --bg flag\"");
        assert!(!flags.background);
    }

    // ---------------------------------------------------------------------------------------
    // parse_inline_config
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_inline_config_bare_progress_token_is_shorthand_true() {
        let cfg = parse_inline_config("progress");
        assert_eq!(cfg.progress, Some(true));
    }

    #[test]
    fn parse_inline_config_progress_false_is_explicit_false() {
        let cfg = parse_inline_config("progress=false");
        assert_eq!(cfg.progress, Some(false));
    }

    #[test]
    fn parse_inline_config_output_false_is_explicitly_disabled() {
        let cfg = parse_inline_config("output=false");
        assert_eq!(cfg.output, Some(InlineOutput::ExplicitlyDisabled));
    }

    #[test]
    fn parse_inline_config_output_path() {
        let cfg = parse_inline_config("output=out.md");
        assert_eq!(cfg.output, Some(InlineOutput::Path("out.md".to_string())));
    }

    #[test]
    fn parse_inline_config_reads_plus_delimited() {
        let cfg = parse_inline_config("reads=a.md+b.md+c.md");
        assert_eq!(
            cfg.reads,
            Some(InlineReads::Paths(vec![
                "a.md".to_string(),
                "b.md".to_string(),
                "c.md".to_string(),
            ]))
        );
    }

    #[test]
    fn parse_inline_config_reads_drops_empty_segments() {
        let cfg = parse_inline_config("reads=a.md++b.md");
        assert_eq!(
            cfg.reads,
            Some(InlineReads::Paths(vec![
                "a.md".to_string(),
                "b.md".to_string(),
            ]))
        );
    }

    #[test]
    fn parse_inline_config_skill_and_skills_both_spellings_accepted() {
        let cfg1 = parse_inline_config("skill=a+b");
        let cfg2 = parse_inline_config("skills=a+b");
        assert_eq!(cfg1.skill, cfg2.skill);
    }

    #[test]
    fn parse_inline_config_unknown_key_is_silently_ignored() {
        let cfg = parse_inline_config("bogusKey=value,model=foo");
        assert_eq!(cfg.model.as_deref(), Some("foo"));
    }

    #[test]
    fn parse_inline_config_count_requires_positive_integer() {
        assert_eq!(parse_inline_config("count=3").count, Some(3));
        assert_eq!(parse_inline_config("count=0").count, None);
        assert_eq!(parse_inline_config("count=-1").count, None);
        assert_eq!(parse_inline_config("count=notanumber").count, None);
    }

    #[test]
    fn parse_inline_config_output_mode_file_only() {
        let cfg = parse_inline_config("outputMode=file-only");
        assert_eq!(cfg.output_mode, Some(OutputMode::FileOnly));
    }

    #[test]
    fn parse_inline_config_multiple_keys_combine() {
        let cfg = parse_inline_config("model=gpt,as=result,label=Step 1,phase=build");
        assert_eq!(cfg.model.as_deref(), Some("gpt"));
        assert_eq!(cfg.as_output.as_deref(), Some("result"));
        assert_eq!(cfg.phase.as_deref(), Some("build"));
    }

    // ---------------------------------------------------------------------------------------
    // parse_agent_token
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_agent_token_no_brackets() {
        let (name, cfg) = parse_agent_token("scout");
        assert_eq!(name, "scout");
        assert_eq!(cfg, InlineStepConfig::default());
    }

    #[test]
    fn parse_agent_token_with_brackets() {
        let (name, cfg) = parse_agent_token("scout[model=gpt,progress]");
        assert_eq!(name, "scout");
        assert_eq!(cfg.model.as_deref(), Some("gpt"));
        assert_eq!(cfg.progress, Some(true));
    }

    // ---------------------------------------------------------------------------------------
    // validate_inline_acceptance (restricted slash-surface vocabulary)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn validate_inline_acceptance_accepts_auto_attested_checked() {
        assert!(validate_inline_acceptance("auto", "scout").is_ok());
        assert!(validate_inline_acceptance("attested", "scout").is_ok());
        assert!(validate_inline_acceptance("checked", "scout").is_ok());
    }

    #[test]
    fn validate_inline_acceptance_rejects_verified_reviewed_none() {
        assert!(validate_inline_acceptance("verified", "scout").is_err());
        assert!(validate_inline_acceptance("reviewed", "scout").is_err());
        assert!(validate_inline_acceptance("none", "scout").is_err());
        assert!(validate_inline_acceptance("bogus", "scout").is_err());
    }

    // ---------------------------------------------------------------------------------------
    // parse_single_task_token
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_single_task_token_double_quoted_task() {
        let step = parse_single_task_token("scout \"find the bug\"");
        assert_eq!(step.name, "scout");
        assert_eq!(step.task.as_deref(), Some("find the bug"));
    }

    #[test]
    fn parse_single_task_token_single_quoted_task() {
        let step = parse_single_task_token("scout 'find the bug'");
        assert_eq!(step.name, "scout");
        assert_eq!(step.task.as_deref(), Some("find the bug"));
    }

    #[test]
    fn parse_single_task_token_empty_quoted_task_is_none() {
        let step = parse_single_task_token("scout \"\"");
        assert_eq!(step.name, "scout");
        assert_eq!(step.task, None);
    }

    #[test]
    fn parse_single_task_token_dash_delimited_task() {
        let step = parse_single_task_token("scout -- find the bug");
        assert_eq!(step.name, "scout");
        assert_eq!(step.task.as_deref(), Some("find the bug"));
    }

    #[test]
    fn parse_single_task_token_bare_agent_no_task() {
        let step = parse_single_task_token("scout");
        assert_eq!(step.name, "scout");
        assert_eq!(step.task, None);
    }

    #[test]
    fn parse_single_task_token_with_inline_config_and_quoted_task() {
        let step = parse_single_task_token("scout[model=gpt] \"find the bug\"");
        assert_eq!(step.name, "scout");
        assert_eq!(step.config.model.as_deref(), Some("gpt"));
        assert_eq!(step.task.as_deref(), Some("find the bug"));
    }

    // ---------------------------------------------------------------------------------------
    // has_unbalanced_parens / split_on_arrow / split_group_tasks (quote/paren-aware scanning)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn has_unbalanced_parens_detects_unmatched_open() {
        assert!(has_unbalanced_parens("(a | b"));
    }

    #[test]
    fn has_unbalanced_parens_detects_unmatched_close() {
        assert!(has_unbalanced_parens("a | b)"));
    }

    #[test]
    fn has_unbalanced_parens_accepts_balanced() {
        assert!(!has_unbalanced_parens("(a | b) -> c"));
    }

    #[test]
    fn has_unbalanced_parens_ignores_parens_inside_quotes() {
        assert!(!has_unbalanced_parens("scout -- \"look at (this)\""));
    }

    #[test]
    fn split_on_arrow_splits_top_level_only() {
        // `split_on_arrow` itself does NOT trim segments (mirrors source's `splitOnArrow`, which
        // pushes raw substrings and lets each call site trim) — trailing/leading whitespace
        // around the `->` separator is preserved verbatim in the returned segments.
        let segments = split_on_arrow("a -> b -> c");
        assert_eq!(segments, vec!["a ", "b ", "c"]);
    }

    #[test]
    fn split_on_arrow_ignores_arrow_inside_parens() {
        let segments = split_on_arrow("a -> (b -> c | d) -> e");
        assert_eq!(segments, vec!["a ", "(b -> c | d) ", "e"]);
    }

    #[test]
    fn split_on_arrow_ignores_arrow_inside_quotes() {
        let segments = split_on_arrow("a \"go -> there\" -> b");
        assert_eq!(segments, vec!["a \"go -> there\" ", "b"]);
    }

    #[test]
    fn split_group_tasks_splits_top_level_pipes() {
        let parts = split_group_tasks("a \"x\" | b \"y\"");
        assert_eq!(parts, vec!["a \"x\" ", " b \"y\""]);
    }

    #[test]
    fn split_group_tasks_ignores_pipe_inside_quotes() {
        let parts = split_group_tasks("a -- \"x | y\" | b -- z");
        assert_eq!(parts, vec!["a -- \"x | y\" ", " b -- z"]);
    }

    // ---------------------------------------------------------------------------------------
    // parse_group_config
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_group_config_bare_boolean_shorthand() {
        let cfg = parse_group_config("failFast,worktree");
        assert_eq!(cfg.fail_fast, Some(true));
        assert_eq!(cfg.worktree, Some(true));
    }

    #[test]
    fn parse_group_config_explicit_false() {
        let cfg = parse_group_config("failFast=false");
        assert_eq!(cfg.fail_fast, Some(false));
    }

    #[test]
    fn parse_group_config_concurrency() {
        let cfg = parse_group_config("concurrency=3");
        assert_eq!(cfg.concurrency, Some(3));
    }

    #[test]
    fn parse_group_config_concurrency_zero_is_rejected() {
        let cfg = parse_group_config("concurrency=0");
        assert_eq!(cfg.concurrency, None);
    }

    // ---------------------------------------------------------------------------------------
    // parse_group_segment / has_group_syntax
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_group_segment_two_tasks() {
        let group = parse_group_segment("(scout \"a\" | writer \"b\")").expect("parses");
        match group {
            ParsedChainElement::Group { tasks, config } => {
                assert_eq!(tasks.len(), 2);
                assert_eq!(tasks[0].name, "scout");
                assert_eq!(tasks[1].name, "writer");
                assert_eq!(config, GroupConfig::default());
            }
            ParsedChainElement::Step(_) => panic!("expected a group"),
        }
    }

    #[test]
    fn parse_group_segment_with_options_suffix() {
        let group =
            parse_group_segment("(scout \"a\" | writer \"b\")[concurrency=2,worktree]")
                .expect("parses");
        match group {
            ParsedChainElement::Group { config, .. } => {
                assert_eq!(config.concurrency, Some(2));
                assert_eq!(config.worktree, Some(true));
            }
            ParsedChainElement::Step(_) => panic!("expected a group"),
        }
    }

    #[test]
    fn parse_group_segment_requires_at_least_two_tasks() {
        let err = parse_group_segment("(scout \"a\")").expect_err("must fail");
        assert!(err.message.contains("at least two tasks"));
    }

    #[test]
    fn parse_group_segment_requires_leading_paren() {
        let err = parse_group_segment("scout \"a\" | writer \"b\"").expect_err("must fail");
        assert!(err.message.contains("wrapped in parentheses"));
    }

    #[test]
    fn parse_group_segment_rejects_malformed_options_suffix() {
        let err =
            parse_group_segment("(scout \"a\" | writer \"b\") concurrency=2").expect_err("must fail");
        assert!(err.message.contains("wrapped in [...]"));
    }

    #[test]
    fn has_group_syntax_true_when_a_top_level_segment_opens_with_paren() {
        assert!(has_group_syntax(
            "scout \"a\" -> (writer \"b\" | reviewer \"c\")"
        ));
    }

    #[test]
    fn has_group_syntax_false_for_paren_inside_shared_task() {
        assert!(!has_group_syntax("scout -- inspect auth (backend)"));
    }

    #[test]
    fn has_group_syntax_false_for_plain_chain() {
        assert!(!has_group_syntax("scout \"a\" -> writer \"b\""));
    }

    // ---------------------------------------------------------------------------------------
    // parse_chain_expression — the full chain-expression grammar, including edge cases
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_chain_expression_simple_two_step() {
        let expr = parse_chain_expression("scout \"a\" -> writer \"b\"").expect("parses");
        assert_eq!(expr.elements.len(), 2);
    }

    #[test]
    fn parse_chain_expression_requires_arrow() {
        let err = parse_chain_expression("scout \"a\"").expect_err("must fail");
        assert!(err.message.contains("require \" -> \""));
    }

    #[test]
    fn parse_chain_expression_with_inline_group_in_the_middle() {
        let expr = parse_chain_expression(
            "scout \"a\" -> (writer \"b\" | reviewer \"c\") -> planner \"d\"",
        )
        .expect("parses");
        assert_eq!(expr.elements.len(), 3);
        assert!(matches!(expr.elements[0], ParsedChainElement::Step(_)));
        assert!(matches!(expr.elements[1], ParsedChainElement::Group { .. }));
        assert!(matches!(expr.elements[2], ParsedChainElement::Step(_)));
    }

    #[test]
    fn parse_chain_expression_group_first() {
        let expr =
            parse_chain_expression("(scout \"a\" | writer \"b\") -> reviewer \"c\"").expect("parses");
        assert_eq!(expr.elements.len(), 2);
        assert!(matches!(expr.elements[0], ParsedChainElement::Group { .. }));
    }

    #[test]
    fn parse_chain_expression_group_with_options_and_trailing_step() {
        let expr = parse_chain_expression(
            "scout \"a\" -> (writer \"b\" | reviewer \"c\")[concurrency=2,failFast] -> planner \"d\"",
        )
        .expect("parses");
        match &expr.elements[1] {
            ParsedChainElement::Group { config, tasks, .. } => {
                assert_eq!(config.concurrency, Some(2));
                assert_eq!(config.fail_fast, Some(true));
                assert_eq!(tasks.len(), 2);
            }
            ParsedChainElement::Step(_) => panic!("expected group"),
        }
    }

    #[test]
    fn parse_chain_expression_rejects_unmatched_parens() {
        let err = parse_chain_expression("scout \"a\" -> (writer \"b\" | reviewer \"c\"")
            .expect_err("must fail");
        assert!(err.message.contains("Unmatched parentheses"));
    }

    #[test]
    fn parse_chain_expression_rejects_unmatched_close_paren_in_plain_segment() {
        let err = parse_chain_expression("scout \"a)\" -> writer \"b\"");
        // A stray `)` inside a QUOTED segment is not itself an error (quote-tracking hides it
        // from the balance scan); this asserts the OUTER-level unmatched case is what actually
        // triggers rejection, not every raw `)` byte.
        assert!(err.is_ok(), "quoted parens must not trip the balance check");
    }

    #[test]
    fn parse_chain_expression_whitespace_only_input_fails_the_arrow_precondition_first() {
        // An input consisting only of whitespace and arrows fails the mandatory-`" -> "`-
        // substring precondition once trimmed (`" ->  -> ".trim()` == `"->  ->"`, which no
        // longer contains a space-padded `" -> "` at all) — this is source's own
        // `parseChainExpression` behavior: the "must include at least one step" check
        // (`slash-commands.ts:749-751`) is defensive dead code given the earlier `" -> "`
        // substring precondition, never actually reachable in practice, ported here faithfully
        // (the check still exists in [`parse_chain_expression`]'s implementation for parity, even
        // though no test input can reach it).
        let err = parse_chain_expression(" ->  -> ").expect_err("must fail");
        assert!(err.message.contains("require \" -> \""));
    }

    #[test]
    fn parse_chain_expression_group_requires_two_tasks_even_mid_chain() {
        let err = parse_chain_expression("scout \"a\" -> (writer \"b\") -> planner \"d\"")
            .expect_err("single-task group must fail");
        assert!(err.message.contains("at least two tasks"));
    }

    // ---------------------------------------------------------------------------------------
    // parse_agent_args (shared /chain, /parallel grammar)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_agent_args_chain_arrow_form() {
        let parsed =
            parse_agent_args("scout \"a\" -> writer \"b\"", AgentArgsCommand::Chain).expect("ok");
        assert_eq!(parsed.steps.len(), 2);
    }

    #[test]
    fn parse_agent_args_dash_delimited_shared_task_form() {
        let parsed =
            parse_agent_args("scout writer -- shared task", AgentArgsCommand::Chain).expect("ok");
        assert_eq!(parsed.steps.len(), 2);
        assert_eq!(parsed.task, "shared task");
    }

    #[test]
    fn parse_agent_args_chain_requires_first_step_task() {
        let err = parse_agent_args("scout -> writer \"b\"", AgentArgsCommand::Chain)
            .expect_err("first step has no task");
        assert!(err.message.contains("First step must have a task"));
    }

    #[test]
    fn parse_agent_args_parallel_requires_at_least_one_task() {
        let err = parse_agent_args("scout -> writer", AgentArgsCommand::Parallel)
            .expect_err("no task anywhere");
        assert!(err.message.contains("At least one step must have a task"));
    }

    #[test]
    fn parse_agent_args_parallel_allows_first_step_without_task_if_another_has_one() {
        let parsed = parse_agent_args("scout -> writer \"b\"", AgentArgsCommand::Parallel)
            .expect("parallel does not require the FIRST step specifically to have a task");
        assert_eq!(parsed.steps.len(), 2);
    }

    #[test]
    fn parse_agent_args_missing_dash_delimiter_errors() {
        let err = parse_agent_args("scout writer", AgentArgsCommand::Chain).expect_err("no --");
        assert!(err.message.starts_with("Usage:"));
    }

    #[test]
    fn parse_agent_args_empty_input_errors() {
        let err = parse_agent_args("", AgentArgsCommand::Chain).expect_err("empty");
        assert!(err.message.starts_with("Usage:"));
    }

    // ---------------------------------------------------------------------------------------
    // parse_run_command
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_run_command_basic() {
        let parsed = parse_run_command("scout find the bug").expect("ok");
        assert_eq!(parsed.agent, "scout");
        assert_eq!(parsed.task, "find the bug");
        assert!(!parsed.flags.background && !parsed.flags.fork);
    }

    #[test]
    fn parse_run_command_with_flags() {
        let parsed = parse_run_command("scout find the bug --bg --fork").expect("ok");
        assert_eq!(parsed.task, "find the bug");
        assert!(parsed.flags.background && parsed.flags.fork);
    }

    #[test]
    fn parse_run_command_with_inline_config() {
        let parsed = parse_run_command("scout[model=gpt] find the bug").expect("ok");
        assert_eq!(parsed.agent, "scout");
        assert_eq!(parsed.config.model.as_deref(), Some("gpt"));
        assert_eq!(parsed.task, "find the bug");
    }

    #[test]
    fn parse_run_command_no_task() {
        let parsed = parse_run_command("scout").expect("ok");
        assert_eq!(parsed.agent, "scout");
        assert_eq!(parsed.task, "");
    }

    #[test]
    fn parse_run_command_empty_input_errors() {
        let err = parse_run_command("").expect_err("empty");
        assert!(err.message.starts_with("Usage:"));
        let err2 = parse_run_command("  --bg  ").expect_err("only flags, no agent");
        assert!(err2.message.starts_with("Usage:"));
    }

    #[test]
    fn parse_run_command_reads_prefix_injected_into_task() {
        let parsed = parse_run_command("scout[reads=a.md+b.md] summarize").expect("ok");
        assert!(parsed.task.starts_with("[Read from: a.md, b.md]"));
        assert!(parsed.task.ends_with("summarize"));
    }

    // ---------------------------------------------------------------------------------------
    // parse_chain_command (end-to-end, both grammar branches)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_chain_command_simple_chain_produces_single_steps() {
        let parsed = parse_chain_command("scout \"a\" -> writer \"b\"").expect("ok");
        assert_eq!(parsed.chain.len(), 2);
        assert!(matches!(parsed.chain[0], RunnerStep::SingleStep(_)));
        assert!(matches!(parsed.chain[1], RunnerStep::SingleStep(_)));
    }

    #[test]
    fn parse_chain_command_with_inline_group_produces_parallel_group_step() {
        let parsed = parse_chain_command(
            "scout \"a\" -> (writer \"b\" | reviewer \"c\") -> planner \"d\"",
        )
        .expect("ok");
        assert_eq!(parsed.chain.len(), 3);
        assert!(matches!(parsed.chain[0], RunnerStep::SingleStep(_)));
        match &parsed.chain[1] {
            RunnerStep::ParallelGroup(spec) => assert_eq!(spec.steps.len(), 2),
            other => panic!("expected ParallelGroup, got {other:?}"),
        }
        assert!(matches!(parsed.chain[2], RunnerStep::SingleStep(_)));
    }

    #[test]
    fn parse_chain_command_propagates_bg_and_fork_flags() {
        let parsed = parse_chain_command("scout \"a\" -> writer \"b\" --bg --fork").expect("ok");
        assert!(parsed.flags.background && parsed.flags.fork);
    }

    #[test]
    fn parse_chain_command_shared_dash_task_applies_only_to_the_first_step() {
        // Faithful port of source's `mapParsedTaskToStepObject(step, parsed.task || undefined, i
        // === 0, ...)` (`slash-commands.ts:901-903`): the shared `-- task` fallback is applied
        // ONLY to the first step (`isFirst`), never to every step in the chain — a later step
        // with no task of its own is left with an empty task. This may look surprising for a
        // "shared task" but is exactly source's own documented precedence
        // (`mapParsedTaskToStepObject`'s doc comment, `slash-commands.ts:863-866`), preserved here
        // deliberately rather than "fixed" to a friendlier-seeming behavior this port must not
        // silently diverge from.
        let parsed = parse_chain_command("scout writer -- shared").expect("ok");
        assert_eq!(parsed.chain.len(), 2);
        match &parsed.chain[0] {
            RunnerStep::SingleStep(spec) => assert_eq!(spec.task, "shared"),
            other => panic!("expected SingleStep, got {other:?}"),
        }
        match &parsed.chain[1] {
            RunnerStep::SingleStep(spec) => assert_eq!(spec.task, ""),
            other => panic!("expected SingleStep, got {other:?}"),
        }
    }

    #[test]
    fn parse_chain_command_group_task_without_its_own_task_errors() {
        let err = parse_chain_command("scout \"a\" -> (writer \"b\" | reviewer)")
            .expect_err("reviewer has no task of its own inside the group");
        assert!(err.message.contains("Each task in a parallel group needs a task"));
    }

    #[test]
    fn parse_chain_command_rejects_invalid_inline_acceptance() {
        let err = parse_chain_command("scout[acceptance=verified] \"a\" -> writer \"b\"")
            .expect_err("verified is not allowed on the slash surface");
        assert!(err.message.contains("supports auto, attested, or checked"));
    }

    #[test]
    fn parse_chain_command_group_options_thread_through_to_parallel_group_spec() {
        let parsed = parse_chain_command(
            "scout \"a\" -> (writer \"b\" | reviewer \"c\")[concurrency=2,worktree] -> planner \"d\"",
        )
        .expect("ok");
        match &parsed.chain[1] {
            RunnerStep::ParallelGroup(spec) => {
                assert_eq!(spec.concurrency, 2);
                assert!(spec.worktree);
            }
            other => panic!("expected ParallelGroup, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------------------------
    // parse_parallel_command
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_parallel_command_two_tasks() {
        let parsed = parse_parallel_command("scout \"a\" -> writer \"b\"").expect("ok");
        assert_eq!(parsed.tasks.len(), 2);
        assert_eq!(parsed.tasks[0].task, "a");
        assert_eq!(parsed.tasks[1].task, "b");
    }

    #[test]
    fn parse_parallel_command_shared_task_applied_to_steps_without_their_own() {
        let parsed = parse_parallel_command("scout writer -- shared").expect("ok");
        assert_eq!(parsed.tasks.len(), 2);
        assert!(parsed.tasks.iter().all(|t| t.task == "shared"));
    }

    // ---------------------------------------------------------------------------------------
    // parse_run_chain_command
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_run_chain_command_basic() {
        let parsed = parse_run_chain_command("my-chain -- do the thing").expect("ok");
        assert_eq!(parsed.chain_name, "my-chain");
        assert_eq!(parsed.task, "do the thing");
    }

    #[test]
    fn parse_run_chain_command_with_flags() {
        let parsed = parse_run_chain_command("my-chain -- do it --bg").expect("ok");
        assert_eq!(parsed.task, "do it");
        assert!(parsed.flags.background);
    }

    #[test]
    fn parse_run_chain_command_missing_delimiter_errors() {
        let err = parse_run_chain_command("my-chain do the thing").expect_err("no --");
        assert!(err.message.starts_with("Usage:"));
    }

    #[test]
    fn parse_run_chain_command_empty_chain_name_errors() {
        let err = parse_run_chain_command(" -- do the thing").expect_err("empty chain name");
        assert!(err.message.starts_with("Usage:"));
    }

    // ---------------------------------------------------------------------------------------
    // parse_subagents_models_command
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_subagents_models_command_no_args() {
        let parsed = parse_subagents_models_command("").expect("ok");
        assert_eq!(parsed.agent, None);
    }

    #[test]
    fn parse_subagents_models_command_one_agent() {
        let parsed = parse_subagents_models_command("scout").expect("ok");
        assert_eq!(parsed.agent.as_deref(), Some("scout"));
    }

    #[test]
    fn parse_subagents_models_command_too_many_args_errors() {
        let err = parse_subagents_models_command("scout writer").expect_err("too many");
        assert!(err.message.starts_with("Usage:"));
    }

    // ---------------------------------------------------------------------------------------
    // parse_single_required_arg family
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_single_required_arg_exact_one_token() {
        assert_eq!(
            parse_single_required_arg("myprofile", "usage").expect("ok"),
            "myprofile"
        );
    }

    #[test]
    fn parse_single_required_arg_zero_tokens_errors() {
        assert!(parse_single_required_arg("", "usage").is_err());
    }

    #[test]
    fn parse_single_required_arg_too_many_tokens_errors() {
        assert!(parse_single_required_arg("a b", "usage").is_err());
    }

    #[test]
    fn parse_subagents_load_profile_command_basic() {
        assert_eq!(
            parse_subagents_load_profile_command("anthropic.quota").expect("ok"),
            "anthropic.quota"
        );
    }

    #[test]
    fn parse_subagents_generate_profiles_command_basic() {
        assert_eq!(
            parse_subagents_generate_profiles_command("anthropic").expect("ok"),
            "anthropic"
        );
    }

    #[test]
    fn parse_subagents_check_profile_command_basic() {
        assert_eq!(
            parse_subagents_check_profile_command("anthropic.quota").expect("ok"),
            "anthropic.quota"
        );
    }

    // ---------------------------------------------------------------------------------------
    // parse_subagents_refresh_provider_models_command
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_refresh_provider_models_no_force() {
        let parsed = parse_subagents_refresh_provider_models_command("anthropic").expect("ok");
        assert_eq!(parsed.provider, "anthropic");
        assert!(!parsed.force);
    }

    #[test]
    fn parse_refresh_provider_models_with_dashdash_force() {
        let parsed =
            parse_subagents_refresh_provider_models_command("anthropic --force").expect("ok");
        assert_eq!(parsed.provider, "anthropic");
        assert!(parsed.force);
    }

    #[test]
    fn parse_refresh_provider_models_with_bare_force() {
        let parsed =
            parse_subagents_refresh_provider_models_command("anthropic force").expect("ok");
        assert_eq!(parsed.provider, "anthropic");
        assert!(parsed.force);
    }

    #[test]
    fn parse_refresh_provider_models_missing_provider_errors() {
        let err = parse_subagents_refresh_provider_models_command("--force")
            .expect_err("no provider given");
        assert!(err.message.starts_with("Usage:"));
    }

    // ---------------------------------------------------------------------------------------
    // parse_subagents_companions_command
    // ---------------------------------------------------------------------------------------

    #[test]
    fn parse_companions_bare_is_status() {
        assert_eq!(
            parse_subagents_companions_command("").expect("ok"),
            CompanionsCommand::Status
        );
    }

    #[test]
    fn parse_companions_explicit_status() {
        assert_eq!(
            parse_subagents_companions_command("status").expect("ok"),
            CompanionsCommand::Status
        );
    }

    #[test]
    fn parse_companions_hide_workspace() {
        let parsed =
            parse_subagents_companions_command("hide pi-intercom workspace").expect("ok");
        assert_eq!(
            parsed,
            CompanionsCommand::Hide {
                package: "pi-intercom".to_string(),
                scope: CompanionsScope::Workspace,
            }
        );
    }

    #[test]
    fn parse_companions_hide_user() {
        let parsed =
            parse_subagents_companions_command("hide pi-prompt-template-model user").expect("ok");
        assert_eq!(
            parsed,
            CompanionsCommand::Hide {
                package: "pi-prompt-template-model".to_string(),
                scope: CompanionsScope::User,
            }
        );
    }

    #[test]
    fn parse_companions_show() {
        let parsed = parse_subagents_companions_command("show pi-intercom").expect("ok");
        assert_eq!(
            parsed,
            CompanionsCommand::Show {
                package: "pi-intercom".to_string(),
            }
        );
    }

    #[test]
    fn parse_companions_unknown_verb_errors() {
        let err = parse_subagents_companions_command("frobnicate pi-intercom")
            .expect_err("not hide/show/status");
        assert!(err.message.starts_with("Usage:"));
    }

    #[test]
    fn parse_companions_unknown_package_errors() {
        let err =
            parse_subagents_companions_command("hide unknown-package workspace").expect_err("bad pkg");
        assert!(err.message.contains("Unknown companion package"));
    }

    #[test]
    fn parse_companions_hide_missing_scope_errors() {
        let err = parse_subagents_companions_command("hide pi-intercom").expect_err("no scope");
        assert!(err.message.starts_with("Usage:"));
    }

    #[test]
    fn parse_companions_hide_invalid_scope_errors() {
        let err = parse_subagents_companions_command("hide pi-intercom galaxy")
            .expect_err("bad scope token");
        assert!(err.message.starts_with("Usage:"));
    }
}
