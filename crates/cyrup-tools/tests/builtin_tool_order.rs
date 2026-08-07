//! Built-in tool ORDER parity with Pi (`coding-agent/src/core/tools/index.ts:147-176`).
//!
//! Registry insertion order is presentation order (`registry.rs` `insert`/`all`/`visible`), and that
//! order is what `cyrup-session-svc/src/builder.rs:671` feeds into `select_active_tools` →
//! `ext_host.active_tools` → `.tools(active_tools)` on the provider request, and into the
//! `Available tools` block of the system prompt. Nothing downstream sorts, so this crate is where
//! the wire order is decided.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_tools::ops::Backend;
use cyrup_tools::registry::{Availability, ToolRegistry};
use cyrup_tools::ToolsOptions;

fn names(tools: &[std::sync::Arc<dyn cyrup_core::Tool>]) -> Vec<&str> {
    tools.iter().map(|t| t.name()).collect()
}

fn registry() -> ToolRegistry {
    ToolRegistry::with_builtins(
        std::env::temp_dir(),
        Backend::default(),
        ToolsOptions::default(),
    )
}

/// Pi `createAllToolDefinitions` (index.ts:156-166) returns its object literal in the order
/// `read, bash, edit, write, grep, find, ls`. `visible(&Availability::All)` is the exact call
/// `cyrup-session-svc/src/builder.rs:648-652` makes to derive `base_tools`.
#[test]
fn visible_all_matches_pi_create_all_tool_definitions_order() {
    let reg = registry();
    let tools = reg.visible(&Availability::All);
    assert_eq!(
        names(&tools),
        ["read", "bash", "edit", "write", "grep", "find", "ls"],
        "wire/prompt tool order must match Pi's createAllToolDefinitions literal"
    );
    // `all()` replays the same `order` vector; keep the two in lockstep.
    assert_eq!(names(&reg.all()), names(&tools), "all() and visible(All) must agree");
}

/// The public constant is used as the built-in membership set (`Availability::NoBuiltins`) but is
/// also the crate's advertised declaration order; Pi has exactly one order, so it must not disagree
/// with the registry.
#[test]
fn builtin_names_constant_agrees_with_registry_order() {
    assert_eq!(
        cyrup_tools::BUILTIN_NAMES.to_vec(),
        names(&registry().all()),
        "BUILTIN_NAMES must not state a different order than the registry hands out"
    );
}

/// Pi `createCodingTools` (index.ts:169-176) is `read, bash, edit, write` — the default active set
/// (`sdk.ts` `defaultActiveToolNames`, `agent-session.ts:2593`). Filtering the registry must
/// reproduce that order, not `read, write, edit, bash`.
#[test]
fn coding_tools_matches_pi_create_coding_tools_order() {
    let tools = cyrup_tools::coding_tools(
        std::env::temp_dir(),
        Backend::default(),
        ToolsOptions::default(),
    );
    assert_eq!(names(&tools), ["read", "bash", "edit", "write"]);
}

/// Pi `createReadOnlyToolDefinitions` (index.ts:147-154) is `read, grep, find, ls`.
#[test]
fn read_only_tools_matches_pi_read_only_order() {
    let tools = cyrup_tools::read_only_tools(
        std::env::temp_dir(),
        Backend::default(),
        ToolsOptions::default(),
    );
    assert_eq!(names(&tools), ["read", "grep", "find", "ls"]);
}
