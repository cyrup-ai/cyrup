//! The prompt as sections: how a built prompt is rendered, and how one prompt is diffed against the
//! one the model already holds (CODE-014).
//!
//! Both functions are pi's, ported from `packages/coding-agent/src/core/system-prompt.ts` @v1.0.0:
//! [`diff_system_prompt_sections`] is `diffSystemPromptSections` (`:204-216`), and
//! [`render_sections`] is the rendering `buildSystemPrompt` (`:195-197`) gets from
//! `getSystemMessageText` — here it IS `cyrup_provider`'s `get_system_message_text`, so the text a
//! caller reads from [`super::SystemPromptBuilder::build`] and the text a provider sends cannot
//! disagree.

use cyrup_core::{Sections, SystemMessage};

/// The one section that is not wrapped in a tag of its own name, and the one a name check reserves
/// (`SystemPromptSections`, `system-prompt.ts:46-50`).
pub const PREAMBLE: &str = "preamble";

/// A prompt's sections as the model reads them: every section's text in order, joined by a blank
/// line (`getSystemMessageText` over `{ content: "", sections }`, `utils/text.ts:15-21`).
pub fn render_sections(sections: &Sections) -> String {
    cyrup_provider::get_system_message_text(&SystemMessage {
        sections: Some(sections.clone()),
        ..SystemMessage::default()
    })
}

/// Pi's `diffSystemPromptSections` (`system-prompt.ts:204-216` @v1.0.0): the `SystemMessage.sections`
/// patch that turns the sections the model currently has (`previous`, replayed from the transcript,
/// so a removed section is simply absent) into the desired `current`, or `None` when nothing changed.
///
/// A section that is new or whose text differs is set to its new text; a section the model has and
/// `current` no longer has is set to `null` (`None` here), which removes it. The patch lists the
/// changes in `current`'s order and then the removals in `previous`'s order, which is the order the
/// two `for` loops upstream build the object in — and the order a later replay applies them in.
///
/// The names are not restricted to the ones cyrup builds: a section pi wrote under a name cyrup has
/// never heard of is in `previous`, is absent from `current`, and so is removed by a `null` in the
/// new row. Nothing rewrites the row that carried it.
pub fn diff_system_prompt_sections(previous: &Sections, current: &Sections) -> Option<Sections> {
    let mut patch = Sections::new();
    for (name, text) in current.iter() {
        // `previous[name] !== text`: an absent name is `undefined`, and so differs.
        if previous.get(name) != Some(text) {
            patch.set(name, text.map(str::to_owned));
        }
    }
    for (name, _) in previous.iter() {
        if current.get(name).is_none() {
            patch.set(name, None);
        }
    }
    (!patch.is_empty()).then_some(patch)
}
