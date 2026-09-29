//! PROV-083a — `utils/text.rs`, the 1:1 port of `packages/ai/src/utils/text.ts` @v0.87.1.

use crate::utils::text::{
    content_text, content_text_default, get_system_message_text, render_system_message_update,
};
use cyrup_core::{Content, Sections, SystemMessage};

fn msg(content: &str, sections: Option<Sections>) -> SystemMessage {
    SystemMessage {
        content: if content.is_empty() {
            Vec::new()
        } else {
            vec![Content::text(content)]
        },
        sections,
        tools_added: Vec::new(),
        tools_removed: Vec::new(),
        timestamp: 0,
    }
}

/// `contentText` (`text.ts:6-12`): TEXT blocks only, joined with the separator.
#[test]
fn content_text_joins_only_text_blocks() {
    let blocks = vec![
        Content::text("a"),
        Content::Image {
            data: "AA".to_string(),
            mime_type: "image/png".to_string(),
        },
        Content::text("b"),
    ];
    assert_eq!(
        content_text_default(&blocks),
        "a\nb",
        "default separator `\\n`"
    );
    assert_eq!(content_text(&blocks, ""), "ab");
    assert_eq!(content_text_default(&[]), "");
}

/// `getSystemMessageText` (`text.ts:15-21`): content, then the non-`null` section VALUES, EMPTY
/// parts dropped, joined `"\n\n"`.
#[test]
fn get_system_message_text_renders_content_then_section_values() {
    assert_eq!(get_system_message_text(&msg("", None)), "");
    assert_eq!(get_system_message_text(&msg("prompt", None)), "prompt");

    let sections = Sections::from_iter([
        ("a".to_string(), Some("alpha".to_string())),
        // A `null` section contributes NOTHING here (`if (text !== null)`, `text.ts:18`).
        ("gone".to_string(), None),
        ("b".to_string(), Some("beta".to_string())),
        // An EMPTY value is dropped by the `part.length > 0` filter (`:20`).
        ("blank".to_string(), Some(String::new())),
    ]);
    assert_eq!(
        get_system_message_text(&msg("prompt", Some(sections.clone()))),
        "prompt\n\nalpha\n\nbeta"
    );
    // With no content the leading empty part is dropped too, not rendered as a blank line.
    assert_eq!(
        get_system_message_text(&msg("", Some(sections))),
        "alpha\n\nbeta"
    );
}

/// `renderSystemMessageUpdate` (`text.ts:28-41`): the section sentences are framed BY NAME, in wire
/// order, and a `null` renders its own removal sentence.
#[test]
fn render_system_message_update_frames_each_section_change_by_name() {
    assert_eq!(render_system_message_update(&msg("", None)), "");
    assert_eq!(render_system_message_update(&msg("more", None)), "more");

    let sections = Sections::from_iter([
        ("tone".to_string(), Some("terse".to_string())),
        ("scope".to_string(), None),
    ]);
    assert_eq!(
        render_system_message_update(&msg("more", Some(sections.clone()))),
        concat!(
            "more\n\n",
            "Updated system prompt section \"tone\":\n\nterse\n\n",
            "Removed system prompt section \"scope\"."
        )
    );
    // Unlike `getSystemMessageText`, the section sentences are pushed UNCONDITIONALLY, so an EMPTY
    // value still renders its `Updated …` frame with an empty body (`text.ts:31-38` has no filter).
    let blank = Sections::from_iter([("s".to_string(), Some(String::new()))]);
    assert_eq!(
        render_system_message_update(&msg("", Some(blank))),
        "Updated system prompt section \"s\":\n\n"
    );
    // And the reverse asymmetry: only `content` is emptiness-filtered here (`:30`).
    assert_eq!(
        render_system_message_update(&msg("", Some(sections))),
        concat!(
            "Updated system prompt section \"tone\":\n\nterse\n\n",
            "Removed system prompt section \"scope\"."
        )
    );
}
