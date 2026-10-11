//! PROMPT-001 — the transcript every adapter's "the prompt reaches the wire" test sends.
//!
//! The agent loop builds each request with [`request_context`] from the transcript it replays, and
//! every adapter reads [`Context::system_prompt`], so a request that carries the transcript's
//! prompt is only proven by serialising what the adapter builds FROM that context. The transcript is
//! what a session writes (`CODE-014`): a leading system row with the prompt as `sections` and an
//! empty `content`, a user turn, and a later system row that patches one section and removes
//! another.

use crate::context::{Context, ToolDef};
use crate::utils::transcript::request_context;
use cyrup_core::{Content, Message, Sections, SystemMessage};

pub(crate) const PREAMBLE: &str = "PREAMBLE-SENTINEL you are the agent";
pub(crate) const TOOLS: &str = "<tools>TOOLS-SENTINEL read, bash</tools>";
pub(crate) const RULES_OLD: &str = "<rules>RULES-SENTINEL-OLD</rules>";
pub(crate) const RULES_NEW: &str = "<rules>RULES-SENTINEL-NEW</rules>";
pub(crate) const DOCS_REMOVED: &str = "<docs>DOCS-SENTINEL</docs>";

fn row(sections: Sections, timestamp: i64) -> Message {
    Message::System(SystemMessage {
        content: Vec::new(),
        sections: Some(sections),
        tools_added: Vec::new(),
        tools_removed: Vec::new(),
        timestamp,
    })
}

/// The persisted transcript: base prompt, a user turn, a later diff row.
pub(crate) fn transcript() -> Vec<Message> {
    let base: Sections = [
        ("preamble", Some(PREAMBLE.to_string())),
        ("tools", Some(TOOLS.to_string())),
        ("rules", Some(RULES_OLD.to_string())),
        ("docs", Some(DOCS_REMOVED.to_string())),
    ]
    .into_iter()
    .collect();
    let diff: Sections = [("rules", Some(RULES_NEW.to_string())), ("docs", None)]
        .into_iter()
        .collect();
    vec![
        row(base, 1),
        Message::User {
            content: vec![Content::text("hi")],
            timestamp: 2,
        },
        row(diff, 3),
    ]
}

/// The request context the agent builds from [`transcript`]: an agent with no prompt of its own,
/// so the transcript IS the prompt. `tools` are the loop's advertised set, passed alongside.
pub(crate) fn agent_context(tools: Vec<ToolDef>) -> Context {
    request_context(Some(""), transcript(), tools)
}

/// Assert the serialised request body carries the replayed prompt, each section once, with the
/// patched section current and the removed one gone.
pub(crate) fn assert_wire_carries_prompt(wire: &str) {
    for (name, text) in [
        ("preamble", PREAMBLE),
        ("tools", TOOLS),
        ("rules", RULES_NEW),
    ] {
        assert_eq!(
            wire.matches(text).count(),
            1,
            "the {name} section must appear exactly once in the request body: {wire}"
        );
    }
    assert!(
        !wire.contains("RULES-SENTINEL-OLD"),
        "the patched section's old text must be gone: {wire}"
    );
    assert!(
        !wire.contains("DOCS-SENTINEL"),
        "a section removed by a later row must be gone: {wire}"
    );
}
