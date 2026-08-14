//! Slash-command registry + dispatch tests (spec/tui/04 §2; gaps 2/19/20).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{CommandRegistry, Dispatch, BUILTIN_SLASH_COMMANDS};

#[test]
fn builtin_table_is_22_commands_in_pi_order() {
    // slash-commands.ts:18-41 — order is display order, NOT alphabetical.
    assert_eq!(BUILTIN_SLASH_COMMANDS.len(), 22);
    assert_eq!(BUILTIN_SLASH_COMMANDS.first().unwrap().name, "settings");
    assert_eq!(BUILTIN_SLASH_COMMANDS[1].name, "model");
    assert_eq!(BUILTIN_SLASH_COMMANDS.last().unwrap().name, "quit");
    // `/model` and `/login` are the two builtins that carry argument completion: at v0.83.0
    // `interactive-mode.ts:553-590` (`createBaseAutocompleteProvider`) installs
    // `getArgumentCompletions` on exactly those two entries, and `autocomplete.ts:342-352` returns
    // `null` for any command without one. Their hints are `slash-commands.ts:21` and `:35`.
    assert!(BUILTIN_SLASH_COMMANDS[1].has_arg_completion);
    assert_eq!(BUILTIN_SLASH_COMMANDS[1].argument_hint.as_deref(), Some("<provider/model>"));
    let with_args: Vec<&str> = BUILTIN_SLASH_COMMANDS
        .iter()
        .filter(|c| c.has_arg_completion)
        .map(|c| c.name.as_ref())
        .collect();
    assert_eq!(with_args, vec!["model", "login"]);
    let login = BUILTIN_SLASH_COMMANDS.iter().find(|c| c.name == "login").unwrap();
    assert_eq!(login.argument_hint.as_deref(), Some("<provider>"));
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
fn theme_think_show_images_route_to_the_agent_like_pi() {
    // Pi has NO `/theme`, `/think`, or `/show-images` builtin (`BUILTIN_SLASH_COMMANDS`,
    // slash-commands.ts:18-90) — each is an unknown `/command` that Pi routes to the AGENT as
    // literal text (interactive-mode.ts onSubmit fallthrough; `isExtensionCommand` returns false).
    // Theme is reached via `/settings` → Theme submenu (settings-selector.ts:603-610); thinking
    // level via Shift+Tab (`app.thinking.cycle`, keybindings.ts). These must therefore dispatch as
    // prompts, never as in-crate commands (regression guard against the reverted 9a703f1 divergence).
    let reg = CommandRegistry::new();
    assert_eq!(reg.dispatch("/theme"), Dispatch::Prompt("/theme".to_string()));
    assert_eq!(reg.dispatch("/think"), Dispatch::Prompt("/think".to_string()));
    assert_eq!(reg.dispatch("/show-images"), Dispatch::Prompt("/show-images".to_string()));
    // ...and none of the three appear in the autocomplete surface.
    for name in ["theme", "think", "show-images"] {
        assert!(reg.commands().iter().all(|c| c.name != name), "/{name} leaked into autocomplete");
    }
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

/// The gap this closes: `CommandSource::{Prompt, Extension, Skill}` were declared and NEVER
/// constructed, `InputEditor::set_registry` had zero callers, and `slash_command_catalog()` — which
/// already merges all three dynamic sources — was consumed only by RPC mode. So an RPC client saw
/// every registered command while the interactive `/` menu showed builtins alone, from the SAME
/// session with the SAME registrations.
#[test]
fn dynamic_commands_from_the_catalog_become_autocomplete_visible() {
    let catalog = vec![
        serde_json::json!({
            "name": "subagent-status",
            "description": "Inspect running subagents",
            "source": "extension",
            "sourceInfo": { "path": "", "source": "extension", "scope": "temporary", "origin": "top-level" },
        }),
        serde_json::json!({
            "name": "review",
            "description": "Review the diff",
            "source": "prompt",
            "sourceInfo": { "path": "/p/.cyrup/prompts/review.md", "source": "local", "scope": "project", "origin": "top-level" },
        }),
        serde_json::json!({
            "name": "deploy",
            "description": "Deploy runbook",
            "source": "skill",
            "sourceInfo": { "path": "/u/.cyrup/skills/deploy", "source": "npm:acme-skills", "scope": "user", "origin": "top-level" },
        }),
        // Not a dynamic command row — must be ignored rather than shown as a bare name.
        serde_json::json!({ "name": "junk", "source": "mystery" }),
    ];

    let dynamic = crate::dynamic_commands_from_catalog(&catalog);
    assert_eq!(dynamic.len(), 3, "the unknown source row is dropped");

    // pi `prefixAutocompleteDescription` (interactive-mode.ts:561-567): `[tag] description`, with
    // the tag from scope (+ package source when there is one).
    assert_eq!(dynamic[0].description, "[t] Inspect running subagents");
    assert_eq!(dynamic[1].description, "[p] Review the diff");
    assert_eq!(dynamic[2].description, "[u:npm:acme-skills] Deploy runbook");

    assert_eq!(dynamic[0].source, crate::CommandSource::Extension);
    assert_eq!(dynamic[1].source, crate::CommandSource::Prompt);
    assert_eq!(dynamic[2].source, crate::CommandSource::Skill);

    let reg = crate::CommandRegistry::with_dynamic(dynamic);
    assert!(
        reg.get("subagent-status").is_some(),
        "a registered extension command must be reachable from the registry"
    );
    // Builtins survive the merge, and still come first in display order.
    assert!(reg.get("model").is_some());
    assert_eq!(reg.commands()[0].source, crate::CommandSource::Builtin);
}

/// A dynamic command is autocomplete-visible but NOT locally dispatchable: it routes to whatever
/// registered it. Merging it into `dispatch_names` would resolve `/subagent-status` to a builtin
/// `Dispatch::Command` that no arm handles.
#[test]
fn a_dynamic_command_is_visible_but_not_locally_dispatched() {
    let catalog = vec![serde_json::json!({
        "name": "subagent-status",
        "description": "Inspect running subagents",
        "source": "extension",
        "sourceInfo": { "path": "", "source": "extension", "scope": "temporary", "origin": "top-level" },
    })];
    let reg = crate::CommandRegistry::with_dynamic(
        crate::dynamic_commands_from_catalog(&catalog),
    );
    assert!(reg.get("subagent-status").is_some(), "visible");
    assert!(
        matches!(reg.dispatch("/subagent-status"), crate::Dispatch::Prompt(_)),
        "not locally dispatched — it falls through to the prompt path, as an unknown slash does"
    );
}

/// A builtin must never be shadowed by a same-named dynamic registration.
#[test]
fn a_builtin_wins_a_name_collision_with_a_dynamic_command() {
    let catalog = vec![serde_json::json!({
        "name": "model",
        "description": "impostor",
        "source": "extension",
        "sourceInfo": { "path": "", "source": "extension", "scope": "temporary", "origin": "top-level" },
    })];
    let reg = crate::CommandRegistry::with_dynamic(
        crate::dynamic_commands_from_catalog(&catalog),
    );
    let model = reg.get("model").expect("builtin /model survives");
    assert_eq!(model.source, crate::CommandSource::Builtin);
    assert_eq!(reg.commands().iter().filter(|c| c.name == "model").count(), 1);
}

/// TUI-025 — the slash-command metadata was one baseline behind.
///
/// pi v0.84.1 `packages/coding-agent/src/core/slash-commands.ts`: `:21`
/// `argumentHint: "<provider/model>"`, `:35` `argumentHint: "<provider>"` on `/login`, and `:40`
/// `"Reload keybindings, extensions, skills, prompts, themes, and context files"`. cyrup carried
/// `"<model>"` (which understates the required `provider/model` form), no `/login` hint at all,
/// and a `/reload` description that omitted context files — which `/reload` does reload.
#[test]
fn the_builtin_command_metadata_matches_pi() {
    let by = |name: &str| {
        crate::BUILTIN_SLASH_COMMANDS
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("`/{name}` must exist"))
    };
    assert_eq!(by("model").argument_hint.as_deref(), Some("<provider/model>"));
    assert_eq!(by("login").argument_hint.as_deref(), Some("<provider>"));
    assert_eq!(
        by("reload").description,
        "Reload keybindings, extensions, skills, prompts, themes, and context files"
    );
    // The `argumentHint` is what `has_arg_completion` reads, so `/login` now advertises one.
    assert!(by("login").has_arg_completion);
    assert!(by("model").has_arg_completion);
}
