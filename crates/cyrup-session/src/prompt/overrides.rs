//! Override resolution result (arch-06 §3.5, R-06-003/004/005).
//!
//! Precedence (CLI > project > global) is resolved upstream by `cyrup-config`; this crate consumes
//! the result.
//!
//! **Append sources REPLACE across tiers and accumulate only WITHIN the CLI tier** (SESS-035's
//! residual; this doc previously said "all of them are joined in precedence order", which upstream
//! does not do). pi's `ResourceLoader.load()` is `let appendSources = this.appendSystemPromptSource;
//! if (!appendSources) { const discovered = this.discoverAppendSystemPromptFile(); appendSources =
//! discovered ? [discovered] : []; }` (`resource-loader.ts:531-535` @v0.83.0), and
//! `discoverAppendSystemPromptFile` (`:1034-1044`) returns **exactly one** path — the trust-gated
//! project `.cyrup/APPEND_SYSTEM.md` if it exists, otherwise the global one, never both. So:
//! any `--append-system-prompt` at all means neither discovered file is consulted, and with no CLI
//! flag at most ONE file is read. The `Vec` this type joins is therefore pi's `appendSources` array
//! — the repeated CLI flags — not a global/project/CLI cascade. Wired at
//! `cyrup-session-svc/src/builder.rs:1244-1252`.

use std::sync::Arc;

/// The precedence-resolved override inputs, computed once per session.
#[derive(Clone, Debug, Default)]
pub struct ResolvedOverride {
    /// Full replacement body. CLI `--system-prompt` wins; else `.cyrup/SYSTEM.md` (project,
    /// trust-gated); else `~/.cyrup/agent/SYSTEM.md` (global). `None` => build the default body.
    pub custom_prompt: Option<Arc<str>>,
    /// Appended text. Repeated `--append-system-prompt` flags win outright and are joined in the
    /// order given (pi's `appendSystemPromptSource` array); with no flag, the SINGLE file
    /// `discoverAppendSystemPromptFile` returns — project `.cyrup/APPEND_SYSTEM.md` under the trust
    /// gate, else global `~/.cyrup/agent/APPEND_SYSTEM.md`. The two files never both contribute
    /// (`resource-loader.ts:531-535` / `:1034-1044` @v0.83.0).
    pub append_system_prompt: Option<Arc<str>>,
}

impl ResolvedOverride {
    /// Join append sources (already in precedence order) into a single `\n\n`-separated block,
    /// dropping empties. Returns `None` when nothing remains (R-06-004).
    pub fn join_appends<I, S>(parts: I) -> Option<Arc<str>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut joined = String::new();
        for p in parts {
            let p = p.as_ref().trim();
            if p.is_empty() {
                continue;
            }
            if !joined.is_empty() {
                joined.push_str("\n\n");
            }
            joined.push_str(p);
        }
        if joined.is_empty() {
            None
        } else {
            Some(Arc::from(joined))
        }
    }
}
