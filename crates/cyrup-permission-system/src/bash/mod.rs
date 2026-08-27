//! Bash command-unit decomposition (port of pi `src/access-intent/bash/`).
//!
//! The bash surface must not be matched as one string: a rule on a leading command would grant the
//! whole chain (`echo hi && rm -rf /` riding an `echo *` allow — pi issues #301/#306). This module
//! parses the command with tree-sitter-bash and enumerates the units the shell will actually run.
//!
//! # Operator-visible policy change
//!
//! **A bash rule written against a whole chain no longer matches.** Before per-unit gating,
//! `{"git add . && git commit *": "allow"}` matched the string `git add . && git commit -m x` and
//! allowed it. That command now enumerates to two units — `git add .` and `git commit -m x` — and
//! the rule matches neither, so both fall through to the category default and the operator's rule
//! never fires (verified: resolves `ask`, `matched_pattern` `None`).
//!
//! This is upstream parity, not a defect: pi's `resolveBashCommandCheck`
//! (`handlers/gates/bash-command.ts:55-105`) falls back to resolving the whole command string ONLY
//! when the unit list is empty. It is called out here because it is silent — such a rule stops
//! taking effect with no error and no warning. Chain rules must be rewritten as one rule per
//! command (`{"git add *": "allow", "git commit *": "allow"}`), which is also the form that
//! actually constrains what runs.

mod enumerate;
mod parser;

pub use enumerate::{BashCommand, BashCommandContext, collect_commands};
pub use parser::parse_command_units;
