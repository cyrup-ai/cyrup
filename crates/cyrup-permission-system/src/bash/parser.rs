//! The tree-sitter-bash parser handle (port of pi `access-intent/bash/parser.ts`).
//!
//! **[CYRUP-DELTA]** pi loads `web-tree-sitter` WASM asynchronously, so it needs `warmBashParser`
//! (`parser.ts:62-70`), a synchronous `getWarmBashParser` accessor, and a cold-path fallback to
//! whole-string matching. The Rust grammar links natively: `Parser::new` + `set_language` measures
//! ~89 µs once and ~6 µs per parse thereafter, so the parser is built lazily per thread and the
//! whole warm-up surface is dropped. There is no cold path and no degraded mode.

use std::cell::RefCell;

use tree_sitter::Parser;

use crate::bash::enumerate::{BashCommand, collect_commands};

thread_local! {
    /// Lazily-built per-thread parser. `None` means the grammar failed to load, in which case the
    /// caller receives `None` and fails closed rather than silently matching the whole string.
    static BASH_PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Parse `command` and enumerate its command units in source order.
///
/// `None` means the grammar failed to load, the parse itself failed, or a unit's source text
/// could not be read ([`collect_commands`] propagates that) — distinct from `Some(vec![])`, which
/// means the command parsed cleanly and legitimately contains nothing to gate (empty,
/// whitespace-only, or comment-only). Callers must fail closed on `None`: a partial unit list
/// would leave a command ungated.
#[must_use]
pub fn parse_command_units(command: &str) -> Option<Vec<BashCommand>> {
    BASH_PARSER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let mut parser = Parser::new();
            // `set_language` fails only on an ABI mismatch between `tree-sitter` and the grammar.
            parser
                .set_language(&tree_sitter_bash::LANGUAGE.into())
                .ok()?;
            *slot = Some(parser);
        }
        let parser = slot.as_mut()?;
        let tree = parser.parse(command, None)?;
        collect_commands(tree.root_node(), command)
    })
}
