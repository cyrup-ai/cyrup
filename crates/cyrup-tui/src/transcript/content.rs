use super::*;

/// Join the `Text` content blocks of a message body into a single string (drops thinking/tool/image
/// blocks). Operates on `cyrup_core::Content`, which is in the dependency set.
pub fn content_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Coalesce the `Thinking` content blocks of a message into one section, joined by `\n\n` — Pi's
/// inner run-collecting loop (`assistant-message.ts:116-127`), which trims each block and skips the
/// empty ones. `redacted` blocks carry no readable text and are dropped with the rest of the empties.
///
/// Pi keeps *runs* of adjacent thinking blocks separate (a text block between two runs starts a new
/// section); cyrup's transcript carries a single reasoning block per turn, so every run of a message
/// folds into one — the difference is only visible when a model interleaves text and thinking.
pub fn thinking_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Thinking { thinking, .. } => Some(thinking.trim()),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A skill block parsed out of a submitted/replayed user message (Pi `ParsedSkillBlock`,
/// agent-session.ts:103).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSkillBlock {
    /// The skill name (`<skill name="…">`).
    pub name: String,
    /// The skill location (`location="…">`) — the on-disk path the skill expanded from.
    pub location: String,
    /// The skill block body (markdown between the open/close tags).
    pub content: String,
    /// The trailing user message after the block, if any (`\n\n{message}`).
    pub user_message: Option<String>,
}

/// Parse a `<skill name="…" location="…">\n…\n</skill>(\n\n{userMessage})?` block out of message text
/// (Pi `parseSkillBlock`, agent-session.ts:114, a hand-port of its anchored regex — no regex dep).
/// Returns `None` for any text that is not exactly such a block.
pub fn parse_skill_block(text: &str) -> Option<ParsedSkillBlock> {
    let rest = text.strip_prefix("<skill name=\"")?;
    let (name, rest) = rest.split_once('"')?;
    if name.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix(" location=\"")?;
    let (location, rest) = rest.split_once('"')?;
    if location.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix(">\n")?;
    // Non-greedy: the body runs to the FIRST `\n</skill>` (`[\s\S]*?`).
    let (content, after) = rest.split_once("\n</skill>")?;
    let content = content.to_string();
    let user_message = if after.is_empty() {
        None
    } else {
        // Must be `\n\n{message}` to end (the regex's optional `(?:\n\n([\s\S]+))?$`).
        let um = after.strip_prefix("\n\n")?.trim();
        (!um.is_empty()).then(|| um.to_string())
    };
    Some(ParsedSkillBlock { name: name.to_string(), location: location.to_string(), content, user_message })
}
