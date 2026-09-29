//! 1:1 port of `packages/ai/src/utils/text.ts` @v0.87.1 (PROV-083a).
//!
//! Three pure renderers over message content: [`content_text`], which joins the text blocks of a
//! content array, and the two system-message renderers [`get_system_message_text`] (the LEADING
//! message, rendered as a complete prompt) and [`render_system_message_update`] (a LATER message,
//! rendered as a framed change for providers that accept system messages mid-conversation).

use cyrup_core::{Content, Sections, SystemMessage};

/// `contentText` (`utils/text.ts:6-12`): extract and join the text of every `text` block.
///
/// Upstream also accepts the bare-string form of `content`; cyrup's message model normalizes that
/// to a single [`Content::Text`] on deserialize, so there is no separate branch — the joined result
/// is identical.
pub fn content_text(content: &[Content], separator: &str) -> String {
    let mut out = String::new();
    let mut first = true;
    for block in content {
        if let Content::Text { text, .. } = block {
            if !first {
                out.push_str(separator);
            }
            out.push_str(text);
            first = false;
        }
    }
    out
}

/// `contentText(content)` with upstream's default separator `"\n"` (`utils/text.ts:6`).
pub fn content_text_default(content: &[Content]) -> String {
    content_text(content, "\n")
}

/// `getSystemMessageText` (`utils/text.ts:15-21`): render a system message as a COMPLETE prompt —
/// its `content` followed by its section values, empty parts dropped, joined with `"\n\n"`.
///
/// A `null` section (a removal) contributes nothing: upstream's `if (text !== null) parts.push(text)`
/// is [`Sections::values`], which skips the `None` entries in wire order.
pub fn get_system_message_text(message: &SystemMessage) -> String {
    join_non_empty(
        std::iter::once(content_text_default(&message.content))
            .chain(section_values(&message.sections).map(str::to_owned)),
    )
}

/// `renderSystemMessageUpdate` (`utils/text.ts:28-41`): render a LATER system message for APIs that
/// accept system messages mid-conversation. Section changes are framed BY NAME so the model can
/// relate them to the leading prompt.
///
/// Upstream's docblock states this framing is request-time only and may change between versions;
/// the two literal sentences are reproduced exactly, because they are wire bytes.
///
/// Note the difference from [`get_system_message_text`]: the empty-part filter applies ONLY to
/// `content` here (upstream pushes the section sentences unconditionally), and a `null` section
/// renders as its own removal sentence rather than being skipped.
pub fn render_system_message_update(message: &SystemMessage) -> String {
    let mut parts: Vec<String> = Vec::new();
    let text = content_text_default(&message.content);
    if !text.is_empty() {
        parts.push(text);
    }
    if let Some(sections) = &message.sections {
        for (name, value) in sections.iter() {
            parts.push(match value {
                None => format!("Removed system prompt section \"{name}\"."),
                Some(v) => format!("Updated system prompt section \"{name}\":\n\n{v}"),
            });
        }
    }
    parts.join("\n\n")
}

/// `Object.values(message.sections ?? {})` filtered to the non-`null` values, in wire order.
fn section_values(sections: &Option<Sections>) -> impl Iterator<Item = &str> {
    sections.iter().flat_map(Sections::values)
}

/// `parts.filter((part) => part.length > 0).join("\n\n")` (`utils/text.ts:20`).
fn join_non_empty(parts: impl Iterator<Item = String>) -> String {
    let mut out = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&part);
    }
    out
}
