#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::editor::*;

// ------------------------------------------------------------- CMDHINT_01 --------------------

/// A registry seeded with one dynamic prompt template carrying an `argument_hint`, mirroring
/// what `dynamic_commands_from_catalog_gated` produces for a real `/flux/aug`-shaped command.
pub(super) fn registry_with_hinted_dynamic(name: &'static str, hint: &'static str) -> CommandRegistry {
    CommandRegistry::with_dynamic(vec![crate::commands::SlashCommand {
        name: std::borrow::Cow::Borrowed(name),
        description: std::borrow::Cow::Borrowed("test command"),
        argument_hint: Some(std::borrow::Cow::Borrowed(hint)),
        source: crate::commands::CommandSource::Prompt,
        has_arg_completion: false,
    }])
}

/// A registry with the same dynamic command but NO argument hint.
fn registry_with_hintless_dynamic(name: &'static str) -> CommandRegistry {
    CommandRegistry::with_dynamic(vec![crate::commands::SlashCommand {
        name: std::borrow::Cow::Borrowed(name),
        description: std::borrow::Cow::Borrowed("test command"),
        argument_hint: None,
        source: crate::commands::CommandSource::Prompt,
        has_arg_completion: false,
    }])
}

// ---- command_highlight: still typing the name (prefix rule) ----------------------------

#[test]
fn still_typing_a_valid_prefix_highlights_the_whole_line_with_no_ghost() {
    let mut ed = InputEditor::new();
    ed.set_text("/mod");
    let h = ed.command_highlight().expect("\"/mod\" is a live prefix of \"model\"");
    assert_eq!(h.token, 0..4);
    assert_eq!(h.ghost, None, "the popup is open; the ghost only appears after an exact match");
}

#[test]
fn a_non_prefix_query_does_not_highlight() {
    let mut ed = InputEditor::new();
    ed.set_text("/zzz");
    assert_eq!(ed.command_highlight(), None);
}

#[test]
fn a_bare_slash_does_not_highlight() {
    let mut ed = InputEditor::new();
    ed.set_text("/");
    assert_eq!(ed.command_highlight(), None, "an empty query is never a prefix confirmation");
}

#[test]
fn text_not_starting_with_slash_never_highlights() {
    let mut ed = InputEditor::new();
    ed.set_text("hello model");
    assert_eq!(ed.command_highlight(), None);
}

#[test]
fn the_highlight_grows_per_keystroke_while_still_a_prefix() {
    let mut ed = InputEditor::new();
    for (text, expect_some) in
        [("/", false), ("/m", true), ("/mo", true), ("/mod", true), ("/model", true)]
    {
        ed.set_text(text);
        assert_eq!(
            ed.command_highlight().is_some(),
            expect_some,
            "{text:?} should{} highlight",
            if expect_some { "" } else { " not" }
        );
    }
}

// ---- command_highlight: exact match + whitespace (freeze rule) --------------------------

#[test]
fn whitespace_after_an_exact_match_freezes_the_token_and_shows_the_ghost() {
    let mut ed = InputEditor::new();
    ed.set_text("/model ");
    let h = ed.command_highlight().expect("exact match on a known builtin");
    assert_eq!(h.token, 0..6, "token is just \"/model\", not the trailing space");
    assert_eq!(h.ghost.as_deref(), Some("<provider/model>"));
}

#[test]
fn the_ghost_disappears_once_a_real_argument_character_is_typed() {
    let mut ed = InputEditor::new();
    ed.set_text("/model ");
    assert!(ed.command_highlight().unwrap().ghost.is_some());
    ed.set_text("/model o");
    let h = ed.command_highlight().expect("the token itself is still an exact match");
    assert_eq!(h.token, 0..6, "the highlight survives continued argument typing");
    assert_eq!(h.ghost, None, "the argument zone is no longer empty");
}

#[test]
fn the_ghost_reappears_when_the_buffer_is_edited_back_to_empty() {
    // No dismissal flag: this is recomputed fresh every call, so re-emptying the argument zone
    // brings the ghost straight back.
    let mut ed = InputEditor::new();
    ed.set_text("/model o");
    assert_eq!(ed.command_highlight().unwrap().ghost, None);
    ed.set_text("/model ");
    assert_eq!(ed.command_highlight().unwrap().ghost.as_deref(), Some("<provider/model>"));
}

#[test]
fn two_spaces_still_count_as_an_empty_argument_zone() {
    let mut ed = InputEditor::new();
    ed.set_text("/model  ");
    assert_eq!(ed.command_highlight().unwrap().ghost.as_deref(), Some("<provider/model>"));
}

#[test]
fn a_prefix_that_is_not_an_exact_command_shows_nothing_after_whitespace() {
    // "/flux " — "flux" is not itself a registered command (only "flux/aug" etc. would be, as
    // dynamic commands), so once whitespace follows, there is no exact match and nothing shows.
    let mut ed = InputEditor::new();
    ed.set_text("/flux ");
    assert_eq!(ed.command_highlight(), None);
}

#[test]
fn an_unknown_command_followed_by_an_argument_shows_nothing() {
    let mut ed = InputEditor::new();
    ed.set_text("/bogus");
    assert_eq!(ed.command_highlight(), None, "\"bogus\" is not a prefix of any builtin");
    ed.set_text("/bogus thing");
    assert_eq!(ed.command_highlight(), None);
}

#[test]
fn dynamic_commands_participate_in_both_rules() {
    let mut ed = InputEditor::new();
    ed.set_registry(registry_with_hinted_dynamic(
        "flux/aug",
        "todo_file | number_of_agents | additional_instructions",
    ));
    // Prefix rule, over a dynamic (non-builtin) name.
    ed.set_text("/flux");
    assert_eq!(ed.command_highlight().unwrap().token, 0..5);
    // Exact match + ghost, over the same dynamic name.
    ed.set_text("/flux/aug ");
    let h = ed.command_highlight().unwrap();
    assert_eq!(h.token, 0..9);
    assert_eq!(
        h.ghost.as_deref(),
        Some("todo_file | number_of_agents | additional_instructions"),
        "the hint is used WHOLE and unsplit, never tokenized on \"|\""
    );
    // `/fa` is a fuzzy subsequence of "flux/aug" but not a real prefix.
    ed.set_text("/fa");
    assert_eq!(ed.command_highlight(), None);
}

#[test]
fn a_hintless_dynamic_command_freezes_with_no_ghost() {
    let mut ed = InputEditor::new();
    ed.set_registry(registry_with_hintless_dynamic("greet"));
    ed.set_text("/greet ");
    let h = ed.command_highlight().expect("exact match");
    assert_eq!(h.ghost, None);
}

// ---- command_highlight: the split_command newline boundary ------------------------------

#[test]
fn a_soft_newline_terminates_the_token_exactly_like_whitespace() {
    // `/flux/aug` + soft-newline + `NOTIFS`: split_command's boundary is the implicit `\n`
    // ending line 0, so this must freeze on `/flux/aug`, not stay in "still typing" mode.
    let mut ed = InputEditor::new();
    ed.set_registry(registry_with_hinted_dynamic("flux/aug", "<hint>"));
    ed.set_text("/flux/aug\nNOTIFS");
    let h = ed.command_highlight().expect("exact match via the newline boundary");
    assert_eq!(h.token, 0..9, "token is \"/flux/aug\" only");
    assert_eq!(h.ghost, None, "line 1 holds a non-whitespace argument, so the zone is not empty");
}

#[test]
fn a_soft_newline_with_an_empty_continuation_still_ghosts() {
    let mut ed = InputEditor::new();
    ed.set_registry(registry_with_hinted_dynamic("flux/aug", "<hint>"));
    // Multi-line buffer whose only continuation line is blank.
    ed.set_text("/flux/aug\n");
    let h = ed.command_highlight().expect("exact match");
    assert_eq!(h.ghost.as_deref(), Some("<hint>"));
}
