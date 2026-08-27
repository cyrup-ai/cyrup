//! Port of pi `system-prompt-sanitizer.ts` — `sanitizeAvailableToolsSection`. Pure string logic with
//! ZERO host/policy dependency (it takes the already-computed exposed-tool set): remove the
//! "Available tools:" section from the system prompt entirely, and drop the "Guidelines:" bullets
//! whose tool is no longer exposed. Ported verbatim (each helper cites its pi `file:line`).

use std::collections::HashSet;

/// pi `SanitizeSystemPromptResult` (`system-prompt-sanitizer.ts:1-4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizeSystemPromptResult {
    /// The sanitized prompt (or the ORIGINAL prompt verbatim when nothing was removed).
    pub prompt: String,
    /// Whether the tools section or any guideline bullet was removed.
    pub removed: bool,
}

const AVAILABLE_TOOLS_SECTION_HEADER: &str = "Available tools:";
const GUIDELINES_SECTION_HEADER: &str = "Guidelines:";

/// A contiguous `[start, end)` run of lines (pi `LineSection`, `system-prompt-sanitizer.ts:6-9`).
#[derive(Debug, Clone, Copy)]
struct LineSection {
    start: usize,
    end: usize,
}

/// pi `TOOL_GUIDELINE_RULES` (`system-prompt-sanitizer.ts:19-58`): for a NORMALIZED guideline line,
/// return `Some(keep)` when a rule matches (keep iff the guideline's tool is still exposed), or `None`
/// when no rule matches (an unrelated bullet is always kept). `allowed` is the exposed-tool set.
fn guideline_keep_rule(normalized: &str, allowed: &HashSet<String>) -> Option<bool> {
    let has = |name: &str| allowed.contains(name);
    match normalized {
        "use bash for file operations like ls, rg, find" => Some(has("bash")),
        "use powershell for file operations like listing, searching, and finding files" => {
            Some(has("powershell"))
        }
        "use bash or powershell for file operations like listing, searching, and finding files" => {
            Some(has("bash") && has("powershell"))
        }
        "prefer grep/find/ls tools over bash for file exploration (faster, respects .gitignore)" => {
            Some(has("bash") && (has("grep") || has("find") || has("ls")))
        }
        "use read to examine files before editing. you must use this tool instead of cat or sed."
        | "use read to examine files instead of cat or sed." => Some(has("read")),
        "use edit for precise changes (old text must match exactly)" => Some(has("edit")),
        "use write only for new files or complete rewrites" => Some(has("write")),
        "when summarizing your actions, output plain text directly - do not use cat or bash to display what you did" => {
            Some(has("edit") || has("write"))
        }
        "use task when work should be delegated to one or more specialized agents instead of handled entirely in the current session." => {
            Some(has("task"))
        }
        "use mcp for mcp discovery first: search by capability, describe one exact tool name, then call it." => {
            Some(has("mcp"))
        }
        _ => None,
    }
}

/// pi `normalizePrompt` (`:60-62`): `\r\n` → `\n`.
fn normalize_prompt(prompt: &str) -> String {
    prompt.replace("\r\n", "\n")
}

/// pi `collapseExtraBlankLines` (`:64-66`): collapse runs of 3+ `\n` to 2, then `trimEnd`.
fn collapse_extra_blank_lines(text: &str) -> String {
    collapse_triple_newlines(text).trim_end().to_string()
}

/// The `\n{3,}` → `\n\n` collapse shared by [`collapse_extra_blank_lines`] and the skills sanitizer
/// (pi's identical `.replace(/\n{3,}/g, "\n\n")`). A run of exactly 2 is preserved; 3+ collapses to 2.
pub(crate) fn collapse_triple_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            run += 1;
        } else {
            if run > 0 {
                out.push_str(if run >= 2 { "\n\n" } else { "\n" });
                run = 0;
            }
            out.push(ch);
        }
    }
    if run > 0 {
        out.push_str(if run >= 2 { "\n\n" } else { "\n" });
    }
    out
}

/// pi `normalizeGuidelineText` (`:68-70`): trim, strip a leading `-`/`*` bullet + its following
/// whitespace (pi `^[-*]\s+`, requiring at least one whitespace after the bullet), collapse internal
/// whitespace runs to single spaces, lowercase.
fn normalize_guideline_text(line: &str) -> String {
    let trimmed = line.trim();
    let stripped = match trimmed.strip_prefix(['-', '*']) {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => trimmed,
    };
    stripped.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// pi `isTopLevelSectionHeader` (`:72-75`): a non-empty trimmed line ending in `:` and not a `-`
/// bullet.
fn is_top_level_section_header(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.ends_with(':') && !t.starts_with('-')
}

/// pi `findSection` (`:77-92`): the `[start, end)` of the section whose header line trims to `header`
/// (end = the next top-level header, else the end of the lines), or `None`.
fn find_section(lines: &[String], header: &str) -> Option<LineSection> {
    let start = lines.iter().position(|l| l.trim() == header)?;
    let mut end = lines.len();
    for index in (start + 1)..lines.len() {
        if lines.get(index).is_some_and(|l| is_top_level_section_header(l)) {
            end = index;
            break;
        }
    }
    Some(LineSection { start, end })
}

/// pi `removeLineSection` (`:94-103`): drop `[section.start, section.end)`, or return the lines
/// unchanged when `section` is `None`.
fn remove_line_section(lines: &[String], section: Option<LineSection>) -> (Vec<String>, bool) {
    match section {
        None => (lines.to_vec(), false),
        Some(s) => {
            let mut out = lines.get(..s.start).unwrap_or(&[]).to_vec();
            out.extend_from_slice(lines.get(s.end..).unwrap_or(&[]));
            (out, true)
        }
    }
}

/// pi `shouldKeepGuideline` (`:105-115`): a matching rule decides; an unmatched bullet is kept.
fn should_keep_guideline(line: &str, allowed: &HashSet<String>) -> bool {
    guideline_keep_rule(&normalize_guideline_text(line), allowed).unwrap_or(true)
}

/// pi `sanitizeGuidelinesSection` (`:117-152`): within the "Guidelines:" section, drop each `- `
/// bullet whose tool is no longer exposed. If every bullet is dropped, drop the whole section
/// (header included); otherwise keep the header + surviving bullets.
fn sanitize_guidelines_section(lines: &[String], allowed: &HashSet<String>) -> (Vec<String>, bool) {
    let Some(section) = find_section(lines, GUIDELINES_SECTION_HEADER) else {
        return (lines.to_vec(), false);
    };

    let before = lines.get(..section.start + 1).unwrap_or(&[]);
    let after = lines.get(section.end..).unwrap_or(&[]);
    let body = lines.get(section.start + 1..section.end).unwrap_or(&[]);

    let filtered_body: Vec<String> = body
        .iter()
        .filter(|line| {
            let trimmed = line.trim();
            // Only `- ` (dash-space) bullets are removal candidates (pi `:128`); anything else is kept.
            if !trimmed.starts_with("- ") {
                return true;
            }
            should_keep_guideline(line, allowed)
        })
        .cloned()
        .collect();

    let removed = filtered_body.len() != body.len();
    if !removed {
        return (lines.to_vec(), false);
    }

    let has_bullet = filtered_body.iter().any(|l| l.trim().starts_with("- "));
    if !has_bullet {
        // Every bullet dropped → drop the whole section (header included), keep what follows (pi
        // `:141-146`).
        let mut out = lines.get(..section.start).unwrap_or(&[]).to_vec();
        out.extend_from_slice(after);
        return (out, true);
    }

    let mut out = before.to_vec();
    out.extend(filtered_body);
    out.extend_from_slice(after);
    (out, true)
}

/// pi `sanitizeAvailableToolsSection` (`system-prompt-sanitizer.ts:154-168`): remove the
/// "Available tools:" section entirely and the denied-tool "Guidelines:" bullets, given the exposed
/// tool set `allowed_tool_names`. Returns the ORIGINAL prompt verbatim (`removed = false`) when
/// neither section changed.
#[must_use]
pub fn sanitize_available_tools_section(
    system_prompt: &str,
    allowed_tool_names: &[String],
) -> SanitizeSystemPromptResult {
    let allowed: HashSet<String> = allowed_tool_names
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    let normalized = normalize_prompt(system_prompt);
    let lines: Vec<String> = normalized.split('\n').map(str::to_string).collect();

    let (after_tools, removed_tools) =
        remove_line_section(&lines, find_section(&lines, AVAILABLE_TOOLS_SECTION_HEADER));
    let (after_guidelines, removed_guidelines) = sanitize_guidelines_section(&after_tools, &allowed);

    let removed = removed_tools || removed_guidelines;
    SanitizeSystemPromptResult {
        prompt: if removed {
            collapse_extra_blank_lines(&after_guidelines.join("\n"))
        } else {
            system_prompt.to_string()
        },
        removed,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn allowed(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn removes_available_tools_section_between_top_level_headers() {
        let prompt = "Intro line.\n\nAvailable tools:\n- bash\n- write\n- read\n\nGuidelines:\n- use read to examine files instead of cat or sed.\n\nNext section:\nbody";
        let out = sanitize_available_tools_section(prompt, &allowed(&["read"]));
        assert!(out.removed);
        // The whole "Available tools:" section is gone.
        assert!(!out.prompt.contains("Available tools:"));
        assert!(!out.prompt.contains("- bash"));
        // A section that follows (a top-level header) is preserved.
        assert!(out.prompt.contains("Next section:"));
        assert!(out.prompt.contains("Intro line."));
    }

    #[test]
    fn strips_denied_tool_guideline_bullet_keeps_allowed() {
        // `write` is NOT exposed, `read` IS. The write-guideline bullet is dropped; read's is kept.
        let prompt = "Guidelines:\n- use read to examine files instead of cat or sed.\n- use write only for new files or complete rewrites\n";
        let out = sanitize_available_tools_section(prompt, &allowed(&["read"]));
        assert!(out.removed);
        assert!(out.prompt.contains("use read to examine files"));
        assert!(
            !out.prompt.contains("use write only for new files"),
            "the denied `write` guideline bullet must be stripped; got:\n{}",
            out.prompt
        );
    }

    #[test]
    fn drops_whole_guidelines_section_when_every_bullet_denied() {
        // Neither `bash` nor `write` exposed → both bullets go → the header goes too.
        let prompt = "Head:\nx\n\nGuidelines:\n- use bash for file operations like ls, rg, find\n- use write only for new files or complete rewrites\n";
        let out = sanitize_available_tools_section(prompt, &allowed(&["read"]));
        assert!(out.removed);
        assert!(!out.prompt.contains("Guidelines:"), "empty guidelines section removed:\n{}", out.prompt);
        assert!(out.prompt.contains("Head:"));
    }

    #[test]
    fn unchanged_prompt_returned_verbatim() {
        // No "Available tools:" section and no removable guideline → original returned, removed=false.
        let prompt = "Just a plain prompt.\nNo sections here.";
        let out = sanitize_available_tools_section(prompt, &allowed(&["read", "bash"]));
        assert!(!out.removed);
        assert_eq!(out.prompt, prompt);
    }

    #[test]
    fn unrelated_bullets_are_kept() {
        let prompt = "Guidelines:\n- always be polite\n- use write only for new files or complete rewrites\n";
        let out = sanitize_available_tools_section(prompt, &allowed(&["read"]));
        assert!(out.removed);
        assert!(out.prompt.contains("always be polite"), "unrelated bullet kept:\n{}", out.prompt);
        assert!(!out.prompt.contains("use write only"));
    }
}
