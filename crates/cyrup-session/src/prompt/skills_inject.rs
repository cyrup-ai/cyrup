//! Skills-section formatting (arch-06 §3.1/§6.1, R-06-010/011).
//!
//! Reuses [`cyrup_resources::SkillPointer`] (name + when-to-use description + on-disk path); the
//! prompt lists skills as short pointers the model expands on demand via the `read` tool (DI-4).
//! The section is gated upstream: emitted only when `read` is available, and skipped entirely when
//! the pointer set is empty (e.g. `--no-skills`).
//!
//! Skills carrying `disable-model-invocation: true` are filtered out HERE rather than at the
//! discovery/assembly site, exactly as Pi does (`formatSkillsForPrompt`, `skills.ts:335-336`): the
//! full pointer set stays intact so the explicit `/skill:name` command remains registered, while the
//! model never learns the skill exists. When every pointer is disabled the section is omitted
//! entirely (`skills.ts:337-339`), not emitted empty.

use cyrup_resources::SkillPointer;

/// Skills section preamble — Pi's five `lines` entries before `<available_skills>`
/// (`skills.ts:342-347`), joined with `\n`. The three prose lines are behavioural instructions, not
/// branding, so they are ported verbatim:
///
/// * `:343` "The following skills provide specialized instructions for specific tasks."
/// * `:344` "Use the read tool to load a skill's file when the task matches its description."
/// * `:345` the relative-path resolution rule — without it, a SKILL.md that references
///   `references/palette.md` or `scripts/run.sh` resolves against the agent's cwd instead of the
///   skill directory, so the model issues reads that miss and the skill silently fails.
///
/// `:346` is the empty string, i.e. the blank line before `<available_skills>`.
const SKILLS_PREAMBLE: &str = "The following skills provide specialized instructions for specific tasks.\n\
    Use the read tool to load a skill's file when the task matches its description.\n\
    When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.\n";

/// Emit the `<available_skills>` block. No-op when `skills` is empty or every skill is
/// model-invocation-disabled.
pub(crate) fn emit_skills_section(out: &mut String, skills: &[SkillPointer]) {
    // Pi `const visibleSkills = skills.filter((s) => !s.disableModelInvocation);` then
    // `if (visibleSkills.length === 0) return "";` (skills.ts:335-339).
    let visible: Vec<&SkillPointer> =
        skills.iter().filter(|s| !s.disable_model_invocation).collect();
    if visible.is_empty() {
        return;
    }
    out.push_str("\n\n");
    out.push_str(SKILLS_PREAMBLE);
    out.push_str("\n<available_skills>\n");
    for s in visible {
        out.push_str("  <skill>\n");
        out.push_str("    <name>");
        push_escaped(out, &s.name);
        out.push_str("</name>\n");
        if let Some(desc) = &s.description {
            out.push_str("    <description>");
            push_escaped(out, desc);
            out.push_str("</description>\n");
        }
        out.push_str("    <location>");
        push_escaped(out, &s.path.to_string_lossy());
        out.push_str("</location>\n");
        out.push_str("  </skill>\n");
    }
    out.push_str("</available_skills>");
}

/// XML-escape `& < > " '` (matches Pi's `escapeXml`), appending to `out`.
fn push_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
}
