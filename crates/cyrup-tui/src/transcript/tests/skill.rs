#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::transcript::*;

#[test]
fn parses_a_skill_block_with_trailing_user_message() {
    let text = "<skill name=\"deploy\" location=\"/skills/deploy.md\">\nRun the deploy steps.\n</skill>\n\nplease deploy prod";
    let block = parse_skill_block(text).expect("should parse");
    assert_eq!(block.name, "deploy");
    assert_eq!(block.location, "/skills/deploy.md");
    assert_eq!(block.content, "Run the deploy steps.");
    assert_eq!(block.user_message.as_deref(), Some("please deploy prod"));
}

#[test]
fn parses_a_skill_block_without_user_message() {
    let text = "<skill name=\"lint\" location=\"/s/lint.md\">\nlint body\nmore\n</skill>";
    let block = parse_skill_block(text).expect("should parse");
    assert_eq!(block.name, "lint");
    assert_eq!(block.content, "lint body\nmore");
    assert_eq!(block.user_message, None);
}

#[test]
fn plain_text_is_not_a_skill_block() {
    assert_eq!(parse_skill_block("just a normal message"), None);
    // A single newline after `</skill>` (not `\n\n`) is not a valid trailer.
    assert_eq!(
        parse_skill_block("<skill name=\"x\" location=\"y\">\nz\n</skill>\noops"),
        None
    );
}

#[test]
fn push_user_splits_a_skill_block_into_two_entries() {
    let mut view = TranscriptView::new();
    view.push_user(
        "<skill name=\"deploy\" location=\"/s/d.md\">\nbody\n</skill>\n\nrun it",
    );
    let entries = view.pending();
    assert!(matches!(entries.first(), Some(Entry::SkillInvocation { name, .. }) if name == "deploy"));
    assert!(matches!(entries.get(1), Some(Entry::User { text, .. }) if text == "run it"));
}

#[test]
fn push_user_keeps_plain_text_as_one_entry() {
    let mut view = TranscriptView::new();
    view.push_user("hello world");
    let entries = view.pending();
    assert_eq!(entries.len(), 1);
    assert!(matches!(entries.first(), Some(Entry::User { text, .. }) if text == "hello world"));
}
