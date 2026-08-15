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
fn only_pis_six_argument_commands_accept_trailing_text() {
    // TUI-074. `setupEditorSubmitHandler` guards exactly six names with
    // `text === "/x" || text.startsWith("/x ")` — model (`:2676`), export (`:2682`), import
    // (`:2687`), name (`:2702`), login (`:2742`), compact (`:2758`) @v0.83.0. The other nineteen
    // are strict equality, so trailing text makes the line a PROMPT upstream. cyrup's matcher was
    // uniform, so `/quit now` quit, `/copy that` copied and `/new session` started a new session.
    let reg = CommandRegistry::new();
    for line in ["/quit now", "/copy that", "/new session", "/trust me", "/tree left", "/debug on"] {
        assert_eq!(
            reg.dispatch(line),
            Dispatch::Prompt(line.to_string()),
            "{line} must reach the agent verbatim, as it does upstream"
        );
    }
    // The six that DO take an argument are unchanged, bare and with one.
    for (line, name, arg) in [
        ("/model claude-opus", "model", Some("claude-opus")),
        ("/export out.html", "export", Some("out.html")),
        ("/import s.jsonl", "import", Some("s.jsonl")),
        ("/name my session", "name", Some("my session")),
        ("/login anthropic", "login", Some("anthropic")),
        ("/compact keep the diff", "compact", Some("keep the diff")),
        ("/compact", "compact", None),
    ] {
        assert_eq!(
            reg.dispatch(line),
            Dispatch::Command { name: name.to_string(), arg: arg.map(str::to_string) },
            "{line}"
        );
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

/// TUI-075 — the `/` menu's dynamic blocks are in pi's display order: prompt templates BEFORE
/// extension commands, skills last (`interactive-mode.ts:625` @v0.83.0,
/// `[...slashCommands, ...templateCommands, ...extensionCommands, ...skillCommandList]`).
///
/// The catalog this list is built from emits the blocks in the opposite order (extensions first,
/// `cyrup-session-svc/src/session.rs:2503`), because it is also the RPC `get_commands` payload; the
/// reorder therefore belongs here, at the one consumer that displays it. Order is user-visible: an
/// empty `/` query returns the list unfiltered on both sides.
#[test]
fn the_dynamic_command_blocks_are_ordered_prompt_then_extension_then_skill() {
    // Catalog order deliberately mirrors `slash_command_catalog()`: extensions, prompts, skills.
    let catalog = vec![
        serde_json::json!({
            "name": "ext-one",
            "description": "first extension command",
            "source": "extension",
            "sourceInfo": { "path": "e", "source": "extension", "scope": "temporary", "origin": "top-level" },
        }),
        serde_json::json!({
            "name": "ext-two",
            "description": "second extension command",
            "source": "extension",
            "sourceInfo": { "path": "e", "source": "extension", "scope": "temporary", "origin": "top-level" },
        }),
        serde_json::json!({
            "name": "review",
            "description": "prompt template",
            "source": "prompt",
            "sourceInfo": { "path": "/p/review.md", "source": "local", "scope": "project", "origin": "top-level" },
        }),
        serde_json::json!({
            "name": "skill:deploy",
            "description": "a skill",
            "source": "skill",
            "sourceInfo": { "path": "/u/deploy", "source": "local", "scope": "user", "origin": "top-level" },
        }),
    ];

    let reg = crate::CommandRegistry::with_dynamic(crate::dynamic_commands_from_catalog(&catalog));
    let dynamic: Vec<(&str, crate::CommandSource)> = reg
        .commands()
        .iter()
        .filter(|c| c.source != crate::CommandSource::Builtin)
        .map(|c| (c.name.as_ref(), c.source))
        .collect();

    assert_eq!(
        dynamic,
        vec![
            ("review", crate::CommandSource::Prompt),
            ("ext-one", crate::CommandSource::Extension),
            ("ext-two", crate::CommandSource::Extension),
            ("skill:deploy", crate::CommandSource::Skill),
        ],
        "pi lists prompt templates before extension commands, and the sort must be STABLE so the \
         catalog's own within-block order (extension LOAD order) survives"
    );
    // Presence before absence: the builtins are still first, so this is a reorder of the dynamic
    // tail rather than a list that lost its head.
    assert_eq!(reg.commands()[0].source, crate::CommandSource::Builtin);
}

/// TUI-085 — a row with NO `sourceInfo` gets NO tag, and its description is passed through
/// unprefixed: pi's `getAutocompleteSourceTag` returns `undefined` for a missing `sourceInfo`
/// (`interactive-mode.ts:498-500` @v0.83.0) and `prefixAutocompleteDescription` then returns the
/// description as-is (`:524-526`).
///
/// cyrup used to default the missing scope to `"temporary"`, rendering `[t] desc` — a provenance
/// claim upstream does not make, and one that may be false. A wrong tag is worse than no tag.
#[test]
fn a_dynamic_command_without_source_info_is_not_tagged() {
    let catalog = vec![
        serde_json::json!({
            "name": "bare",
            "description": "no provenance at all",
            "source": "extension",
        }),
        // The control: an identical row WITH `sourceInfo` still gets its tag, so the assertion
        // above is about the missing key and not about tagging having been switched off.
        serde_json::json!({
            "name": "tagged",
            "description": "has provenance",
            "source": "extension",
            "sourceInfo": { "path": "e", "source": "extension", "scope": "project", "origin": "top-level" },
        }),
    ];

    let dynamic = crate::dynamic_commands_from_catalog(&catalog);
    assert_eq!(dynamic.len(), 2);
    assert_eq!(dynamic[0].description, "no provenance at all");
    assert_eq!(dynamic[1].description, "[p] has provenance");
}

/// TUI-079 — `getPathCommandArgument` (`interactive-mode.ts:5450-5477` @v0.83.0), all four cases.
///
/// The defect this pins is silent: `/export "my session.html"` used to write a file whose name
/// literally contained the quote characters, because dispatch handed the whole trimmed remainder
/// through as the path. Quoting a path that contains spaces is the first thing a user tries.
#[test]
fn a_path_command_argument_is_one_quote_aware_token() {
    let arg = crate::commands::path_command_argument;

    // Quoted: the quotes are stripped and the inner spaces survive.
    assert_eq!(arg("\"my session.html\"").as_deref(), Some("my session.html"));
    assert_eq!(arg("'my session.html'").as_deref(), Some("my session.html"));
    // Unquoted: the token ends at the first whitespace — a second word is NOT part of the path.
    assert_eq!(arg("a.html junk").as_deref(), Some("a.html"));
    assert_eq!(arg("a.html").as_deref(), Some("a.html"));
    // An unterminated quote is a REFUSAL upstream (`return undefined`), not a best-effort path.
    assert_eq!(arg("\"a b"), None);
    assert_eq!(arg("'a b"), None);
    // Empty / whitespace-only is the no-argument case.
    assert_eq!(arg(""), None);
    assert_eq!(arg("   "), None);
    // A quote INSIDE an unquoted token is not special — only a LEADING quote opens a quoted token.
    assert_eq!(arg("a\"b.html").as_deref(), Some("a\"b.html"));
}
