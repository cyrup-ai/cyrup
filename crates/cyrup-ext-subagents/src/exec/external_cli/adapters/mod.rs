//! SUBA-074 stage 2 — the code-owned external-CLI adapters.
//!
//! **Why a closed enum and free functions rather than `Box<dyn ExternalCliAdapter>`.** The obvious
//! Rust shape for "three adapters, one interface" is a trait with a registry. It is rejected here on
//! a security argument, not an aesthetic one: these adapters are code-owned BY DESIGN.
//! `validateCodeOwnedProfileRunner` (`external-cli-contract.ts:48-63`, ported at
//! [`crate::runner::contract::validate_code_owned_profile_runner`]) reserves the selection names
//! `claude-code`/`codex-exec`/`cursor-agent` precisely so nothing else can present itself as the
//! sandboxed read-only profile, and `parseExternalCliCapabilityNarrowing` exists so user config can
//! only narrow what code owns. An open trait — even a crate-private one — invites an extension point
//! that reopens both. A closed [`crate::runner::contract::AdapterId`] plus free `launch` functions
//! matches upstream's own structure and keeps "the set of adapters" a fact the compiler knows.
//!
//! Testability does not argue the other way: upstream tests its adapters through
//! `commandPrefixArgs` — a fake PROCESS, not a fake adapter (`claude-code-adapter.ts:84-85`) — and
//! that seam is ported on [`super::ExternalCliLaunchContext::command_prefix_args`].
//!
//! **Shipped in this batch:** [`claude_code`] (`claude-code` and `claude-code-writer`) and the
//! generic no-adapter path. `codex-exec` and `cursor-agent` remain refused by
//! [`crate::runner::dispatch::resolve_runner_dispatch`]; see that module for why.

pub mod claude_code;

use super::framing::{ParserProgress, ParserTerminal};

/// The parsers this build owns, as a closed enum.
///
/// Adding a variant is how a new adapter's stream vocabulary arrives; there is no way to register
/// one from outside the crate.
#[derive(Debug)]
pub enum AdapterParser {
    /// `createClaudeCodeJsonlParser` (`claude-code-adapter.ts:57-78`).
    ClaudeCode(claude_code::ClaudeCodeParser),
}

impl AdapterParser {
    /// Feed one JSONL line.
    ///
    /// # Errors
    ///
    /// The adapter's own protocol error — a malformed event, or one that violates its
    /// after-terminal policy.
    pub fn parse_line(&mut self, line: &str) -> Result<ParserProgress, String> {
        match self {
            Self::ClaudeCode(parser) => parser.parse_line(line),
        }
    }

    /// The bounded-prefix hook for a line too long to buffer whole
    /// (`external-cli-runner.ts:44-45`). `None` means "this adapter cannot skip it", which fails
    /// the parse. Only cursor-agent implements it upstream, and cursor-agent is deferred, so every
    /// shipped arm returns `None`.
    #[must_use]
    pub fn skip_oversized_line(
        &mut self,
        _prefix: &str,
        _byte_length: usize,
    ) -> Option<ParserProgress> {
        match self {
            Self::ClaudeCode(_) => None,
        }
    }

    /// The parser's terminal state, or `None` if it never reached one (`:371-372`, which fails the
    /// run).
    #[must_use]
    pub fn finish(&mut self) -> Option<ParserTerminal> {
        match self {
            Self::ClaudeCode(parser) => parser.finish(),
        }
    }
}
