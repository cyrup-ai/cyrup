//! Slash-command registry + dispatch tests (spec/tui/04 §2; gaps 2/19/20).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::{CommandRegistry, Dispatch, BUILTIN_SLASH_COMMANDS};

#[test]
fn builtin_table_is_22_commands_in_pi_order() {
    // slash-commands.ts:18-41 — order is display order, NOT alphabetical.
    assert_eq!(BUILTIN_SLASH_COMMANDS.len(), 22);
    assert_eq!(BUILTIN_SLASH_COMMANDS.first().unwrap().name, "settings");
    assert_eq!(BUILTIN_SLASH_COMMANDS[1].name, "model");
    assert_eq!(BUILTIN_SLASH_COMMANDS.last().unwrap().name, "quit");
    // Only /model carries argument completion (§2.2 / edge 4).
    assert!(BUILTIN_SLASH_COMMANDS[1].has_arg_completion);
    assert_eq!(BUILTIN_SLASH_COMMANDS[1].argument_hint, Some("<model>"));
    assert!(BUILTIN_SLASH_COMMANDS.iter().filter(|c| c.has_arg_completion).count() == 1);
}

#[test]
fn dispatch_exact_command() {
    let reg = CommandRegistry::new();
    assert_eq!(
        reg.dispatch("/tree"),
        Dispatch::Command { name: "tree".to_string(), arg: None }
    );
}

#[test]
fn dispatch_command_with_argument() {
    let reg = CommandRegistry::new();
    assert_eq!(
        reg.dispatch("/model claude-opus"),
        Dispatch::Command { name: "model".to_string(), arg: Some("claude-opus".to_string()) }
    );
    // Trailing whitespace arg trims to None.
    assert_eq!(
        reg.dispatch("/compact   "),
        Dispatch::Command { name: "compact".to_string(), arg: None }
    );
}

#[test]
fn modelx_is_not_model_command_falls_through_to_prompt() {
    // Edge 1 (interactive-mode.ts:2565): exact-or-`"name "`-prefix only.
    let reg = CommandRegistry::new();
    assert_eq!(reg.dispatch("/modelfoo"), Dispatch::Prompt("/modelfoo".to_string()));
}

#[test]
fn unknown_slash_is_a_prompt_not_an_error() {
    // Edge 2: unknown `/foo` is sent to the agent as literal text.
    let reg = CommandRegistry::new();
    assert_eq!(reg.dispatch("/nope"), Dispatch::Prompt("/nope".to_string()));
}

#[test]
fn hidden_commands_dispatch_but_are_not_in_autocomplete() {
    let reg = CommandRegistry::new();
    assert_eq!(
        reg.dispatch("/debug"),
        Dispatch::Command { name: "debug".to_string(), arg: None }
    );
    // …but they are not listed in the autocomplete-visible commands.
    assert!(reg.commands().iter().all(|c| c.name != "debug"));
    assert!(reg.commands().iter().all(|c| c.name != "arminsayshi"));
}

#[test]
fn bash_precedence_after_slash_before_prompt() {
    // §2.4: `!cmd` included, `!!cmd` excluded.
    let reg = CommandRegistry::new();
    assert_eq!(
        reg.dispatch("!cargo test"),
        Dispatch::Bash { command: "cargo test".to_string(), excluded: false }
    );
    assert_eq!(
        reg.dispatch("!!secret-cmd"),
        Dispatch::Bash { command: "secret-cmd".to_string(), excluded: true }
    );
    // Empty bash body falls through to normal text.
    assert_eq!(reg.dispatch("!  "), Dispatch::Prompt("!".to_string()));
}

#[test]
fn whitespace_only_is_empty() {
    let reg = CommandRegistry::new();
    assert_eq!(reg.dispatch("   "), Dispatch::Empty);
}

#[test]
fn plain_text_is_a_prompt() {
    let reg = CommandRegistry::new();
    assert_eq!(reg.dispatch("hello there"), Dispatch::Prompt("hello there".to_string()));
}
