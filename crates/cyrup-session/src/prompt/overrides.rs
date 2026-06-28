//! Override resolution result (arch-06 §3.5, R-06-003/004/005).
//!
//! Precedence (CLI > project > global) is resolved upstream by `cyrup-config`; this crate consumes
//! the result. Append sources **accumulate** (they do not override): all of them are joined in
//! precedence order into one block.

use std::sync::Arc;

/// The precedence-resolved override inputs, computed once per session.
#[derive(Clone, Debug, Default)]
pub struct ResolvedOverride {
    /// Full replacement body. CLI `--system-prompt` wins; else `.cyrup/SYSTEM.md` (project,
    /// trust-gated); else `~/.cyrup/agent/SYSTEM.md` (global). `None` => build the default body.
    pub custom_prompt: Option<Arc<str>>,
    /// Appended text: ALL append sources joined in precedence order (global `APPEND_SYSTEM.md`,
    /// project `APPEND_SYSTEM.md`, then each repeated `--append-system-prompt`).
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
