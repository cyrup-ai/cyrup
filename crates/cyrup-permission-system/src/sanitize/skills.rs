//! Port of pi `skill-prompt-sanitizer.ts` — `resolveSkillPromptEntries` (and its parse). The system
//! prompt advertises skills in `<available_skills>` blocks; a skill the policy resolves to `ask`/`deny`
//! must NOT be advertised (advertising a capability the gate would only block pollutes the context and
//! invites the model to attempt it), yet its ENFORCEMENT entry must survive so the `before_tool_call`
//! skill-read gate (`extension.rs` `resolve_skill_read` → `skill::find_skill_path_match`) still governs
//! reads of its files. This module does ONE parse of the `<available_skills>` blocks and feeds BOTH
//! consumers from it (pi's `resolveSkillPromptEntries` returns `{ entries, prompt }`, `index.ts:2175`):
//! the flat enforcement entry list AND the hide-sanitized prompt.

use serde_json::json;

use crate::manager::PermissionManager;
use crate::skill::{resolved_entry, SkillPromptEntry};
use crate::types::PermissionState;

const AVAILABLE_SKILLS_OPEN_TAG: &str = "<available_skills>";
const AVAILABLE_SKILLS_CLOSE_TAG: &str = "</available_skills>";

/// The result of [`resolve_skill_prompt_entries`] (pi `{ prompt, entries }`,
/// `skill-prompt-sanitizer.ts:225`).
#[derive(Debug, Clone)]
pub struct SkillPromptResolution {
    /// The hide-sanitized system prompt (`ask`/`deny` skills removed from `<available_skills>` +
    /// structured references pruned). Equal to the input when nothing was hidden.
    pub prompt: String,
    /// The FLAT enforcement entries (every parsed skill, allowed or not) the skill-read gate uses.
    pub entries: Vec<SkillPromptEntry>,
}

/// A raw parsed `<skill>` block (pi `ParsedSkillPromptEntry`, `skill-prompt-sanitizer.ts:14-18`).
#[derive(Debug, Clone)]
struct ParsedSkillEntry {
    name: String,
    description: String,
    location: String,
}

/// One `<available_skills>…</available_skills>` section (pi `SkillPromptSection`, `:29-33`). `start`
/// and `end` are BYTE offsets into the prompt (`start` = the open tag, `end` = just after the close).
#[derive(Debug, Clone)]
struct SkillPromptSection {
    start: usize,
    end: usize,
    entries: Vec<ParsedSkillEntry>,
}

/// pi `decodeXml` (`:35-42`): decode the five entity references, `&amp;` LAST (so a decoded `&lt;` is
/// not re-scanned).
fn decode_xml(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// pi `encodeXml` (`:44-51`): encode the five characters, `&` FIRST (so encoded `&lt;` is not
/// re-encoded).
fn encode_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Extract the inner text of the FIRST `<tag>…</tag>` in `block` (pi's per-field
/// `/<tag>([\s\S]*?)<\/tag>/` non-greedy match), or `None` if the pair is absent.
fn extract_tag<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let rest = block.get(start..)?;
    let end = rest.find(&close)?;
    rest.get(..end)
}

/// pi `parseSkillEntries` (`skill-prompt-sanitizer.ts:53-79`): each `<skill>…</skill>` block carrying
/// `<name>`, `<description>` AND `<location>` tags (name + location non-empty) becomes a parsed entry.
fn parse_skill_entries(section_body: &str) -> Vec<ParsedSkillEntry> {
    let mut entries: Vec<ParsedSkillEntry> = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel_open) = section_body.get(cursor..).and_then(|s| s.find("<skill>")) {
        let block_start = cursor + rel_open + "<skill>".len();
        let Some(rest) = section_body.get(block_start..) else { break };
        let Some(rel_close) = rest.find("</skill>") else { break };
        let block = rest.get(..rel_close).unwrap_or("");
        cursor = block_start + rel_close + "</skill>".len();

        // pi requires all three tags present (`:63`); name + location must be non-empty (`:71`).
        let (Some(name_raw), Some(description_raw), Some(location_raw)) =
            (extract_tag(block, "name"), extract_tag(block, "description"), extract_tag(block, "location"))
        else {
            continue;
        };
        let name = decode_xml(name_raw.trim());
        let description = decode_xml(description_raw.trim());
        let location = decode_xml(location_raw.trim());
        if name.is_empty() || location.is_empty() {
            continue;
        }
        entries.push(ParsedSkillEntry { name, description, location });
    }
    entries
}

/// pi `parseAllSkillPromptSections` (`skill-prompt-sanitizer.ts:102-128`): every
/// `<available_skills>…</available_skills>` section (with byte boundaries + parsed entries), in order.
/// This is THE single parse both consumers share.
fn parse_all_skill_prompt_sections(prompt: &str) -> Vec<SkillPromptSection> {
    let mut sections: Vec<SkillPromptSection> = Vec::new();
    let mut search_start = 0usize;
    while let Some(rel_open) =
        prompt.get(search_start..).and_then(|s| s.find(AVAILABLE_SKILLS_OPEN_TAG))
    {
        let start = search_start + rel_open;
        let body_start = start + AVAILABLE_SKILLS_OPEN_TAG.len();
        let Some(rest) = prompt.get(body_start..) else { break };
        let Some(rel_close) = rest.find(AVAILABLE_SKILLS_CLOSE_TAG) else { break };
        let close_start = body_start + rel_close;
        let end = close_start + AVAILABLE_SKILLS_CLOSE_TAG.len();
        let body = prompt.get(body_start..close_start).unwrap_or("");
        sections.push(SkillPromptSection { start, end, entries: parse_skill_entries(body) });
        search_start = end;
    }
    sections
}

/// pi `renderAvailableSkillsSection` (`:161-173`): re-render a section from scratch (2-space indent,
/// XML-encoded fields) carrying ONLY the still-visible (allowed) skills.
fn render_available_skills_section(entries: &[&ParsedSkillEntry]) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(entries.len() * 5 + 2);
    lines.push(AVAILABLE_SKILLS_OPEN_TAG.to_string());
    for e in entries {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", encode_xml(&e.name)));
        lines.push(format!("    <description>{}</description>", encode_xml(&e.description)));
        lines.push(format!("    <location>{}</location>", encode_xml(&e.location)));
        lines.push("  </skill>".to_string());
    }
    lines.push(AVAILABLE_SKILLS_CLOSE_TAG.to_string());
    lines.join("\n")
}

/// pi `removePromptRange` (`:175-179`): splice out `[start, end)`, dropping trailing newlines before
/// the removed range so an emptied section does not leave a blank gap.
fn remove_prompt_range(prompt: &str, start: usize, end: usize) -> String {
    let before = prompt.get(..start).unwrap_or("").trim_end_matches('\n');
    let after = prompt.get(end..).unwrap_or("");
    format!("{before}{after}")
}

/// pi `isStructuredSkillReferenceLine` (`:196-199`): a table row (`|…`) or a `-`/`*`/`+` list item.
fn is_structured_skill_reference_line(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with('|') {
        return true;
    }
    // pi `/^[-*+]\s+/`: a bullet char immediately followed by whitespace.
    matches!(t.strip_prefix(['-', '*', '+']), Some(rest) if rest.starts_with(char::is_whitespace))
}

/// pi `lineContainsBacktickedHiddenSkill` (`:181-194`): does the line backtick-quote any hidden skill
/// name (`` `name` ``, XML-decoded)?
fn line_contains_backticked_hidden_skill(line: &str, hidden: &[String]) -> bool {
    if hidden.is_empty() || !line.contains('`') {
        return false;
    }
    let mut rest = line;
    while let Some(open) = rest.find('`') {
        let after_open = rest.get(open + 1..).unwrap_or("");
        let Some(close) = after_open.find('`') else { break };
        let inner = after_open.get(..close).unwrap_or("");
        let name = decode_xml(inner.trim());
        if !name.is_empty() && hidden.iter().any(|h| h == &name) {
            return true;
        }
        rest = after_open.get(close + 1..).unwrap_or("");
    }
    false
}

/// pi `pruneHiddenStructuredSkillReferences` (`:201-218`): drop table rows / list items that
/// backtick-reference a hidden skill, then collapse any 3+ blank-line run left behind.
fn prune_hidden_structured_skill_references(prompt: &str, hidden: &[String]) -> String {
    if hidden.is_empty() {
        return prompt.to_string();
    }
    let mut removed = false;
    let pruned: Vec<&str> = prompt
        .split('\n')
        .filter(|line| {
            if is_structured_skill_reference_line(line)
                && line_contains_backticked_hidden_skill(line, hidden)
            {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect();
    if removed {
        crate::sanitize::tools::collapse_triple_newlines(&pruned.join("\n"))
    } else {
        prompt.to_string()
    }
}

/// pi `resolveSkillPromptEntries` (`skill-prompt-sanitizer.ts:220-278`): parse the `<available_skills>`
/// block(s) ONCE, resolve each entry's state via `check_permission("skill", {name}, agent)`, and
/// produce BOTH (a) the flat enforcement entries the skill-read gate uses AND (b) the hide-sanitized
/// prompt (only `allow` skills stay visible; `ask`/`deny` skills are removed from the block and any
/// structured backtick reference to them is pruned). A per-name cache mirrors pi's `permissionCache`.
#[must_use]
pub fn resolve_skill_prompt_entries(
    prompt: &str,
    manager: &mut PermissionManager,
    agent_name: Option<&str>,
    cwd: &str,
) -> SkillPromptResolution {
    let sections = parse_all_skill_prompt_sections(prompt);
    if sections.is_empty() {
        return SkillPromptResolution { prompt: prompt.to_string(), entries: Vec::new() };
    }

    let mut cache: Vec<(String, PermissionState)> = Vec::new();
    let mut enforcement: Vec<SkillPromptEntry> = Vec::new();
    let mut hidden_names: Vec<String> = Vec::new();
    // `(start, end, replacement)` in section order; applied in REVERSE so earlier offsets stay valid.
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();

    for section in &sections {
        let mut visible: Vec<&ParsedSkillEntry> = Vec::new();
        for entry in &section.entries {
            let state = match cache.iter().find(|(n, _)| *n == entry.name) {
                Some((_, s)) => *s,
                None => {
                    let s = manager
                        .check_permission("skill", &json!({ "name": entry.name }), agent_name)
                        .state;
                    cache.push((entry.name.clone(), s));
                    s
                }
            };
            enforcement.push(resolved_entry(entry.name.clone(), &entry.location, state, cwd));
            if state == PermissionState::Allow {
                visible.push(entry);
            } else if !hidden_names.iter().any(|n| n == &entry.name) {
                hidden_names.push(entry.name.clone());
            }
        }

        // The whole section is allow-visible → no replacement needed (pi `:253-255`).
        if visible.len() == section.entries.len() {
            continue;
        }
        let content = if visible.is_empty() {
            String::new()
        } else {
            render_available_skills_section(&visible)
        };
        replacements.push((section.start, section.end, content));
    }

    let mut sanitized = prompt.to_string();
    for (start, end, content) in replacements.iter().rev() {
        sanitized = if content.is_empty() {
            remove_prompt_range(&sanitized, *start, *end)
        } else {
            let before = sanitized.get(..*start).unwrap_or("");
            let after = sanitized.get(*end..).unwrap_or("");
            format!("{before}{content}{after}")
        };
    }
    sanitized = prune_hidden_structured_skill_references(&sanitized, &hidden_names);

    SkillPromptResolution { prompt: sanitized, entries: enforcement }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::manager::ManagerPaths;
    use std::path::Path;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    fn manager_with_global(dir: &Path, body: &str) -> PermissionManager {
        let global = dir.join("cyrup-permissions.jsonc");
        write(&global, body);
        PermissionManager::new(ManagerPaths {
            global_config_path: global,
            agents_dir: dir.join("agents"),
            project_global_config_path: None,
            project_agents_dir: None,
            legacy_global_settings_path: dir.join("settings.json"),
            global_mcp_config_path: dir.join("mcp.json"),
            mcp_server_names_override: Some(Vec::new()),
        })
    }

    fn skill_block(name: &str, loc: &str) -> String {
        format!(
            "  <skill>\n    <name>{name}</name>\n    <description>d-{name}</description>\n    <location>{loc}</location>\n  </skill>"
        )
    }

    #[test]
    fn parses_all_entries_as_enforcement_regardless_of_state() {
        let dir = tempfile::tempdir().unwrap();
        // `deploy` allow, `secret` deny.
        let mut m = manager_with_global(
            dir.path(),
            r#"{ "skills": { "deploy": "allow", "secret": "deny" } }"#,
        );
        let prompt = format!(
            "head\n<available_skills>\n{}\n{}\n</available_skills>\ntail",
            skill_block("deploy", "/x/skills/deploy/SKILL.md"),
            skill_block("secret", "/x/skills/secret/SKILL.md"),
        );
        let out = resolve_skill_prompt_entries(&prompt, &mut m, None, "/x");
        // BOTH skills survive as enforcement entries...
        assert_eq!(out.entries.len(), 2);
        assert!(out.entries.iter().any(|e| e.name == "deploy" && e.state == PermissionState::Allow));
        assert!(out.entries.iter().any(|e| e.name == "secret" && e.state == PermissionState::Deny));
    }

    #[test]
    fn hides_non_allow_skill_but_keeps_allowed_visible() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(
            dir.path(),
            r#"{ "skills": { "deploy": "allow", "secret": "deny" } }"#,
        );
        let prompt = format!(
            "head\n<available_skills>\n{}\n{}\n</available_skills>\ntail",
            skill_block("deploy", "/x/skills/deploy/SKILL.md"),
            skill_block("secret", "/x/skills/secret/SKILL.md"),
        );
        let out = resolve_skill_prompt_entries(&prompt, &mut m, None, "/x");
        // ...but only the ALLOW skill stays advertised in the sanitized prompt.
        assert!(out.prompt.contains("<name>deploy</name>"), "allowed skill visible:\n{}", out.prompt);
        assert!(
            !out.prompt.contains("<name>secret</name>"),
            "denied skill must be hidden from <available_skills>:\n{}",
            out.prompt
        );
        assert!(out.prompt.contains("head") && out.prompt.contains("tail"));
    }

    #[test]
    fn removes_whole_block_when_no_skill_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(dir.path(), r#"{ "skills": { "*": "ask" } }"#);
        let prompt = format!(
            "before\n\n<available_skills>\n{}\n</available_skills>\n\nafter",
            skill_block("deploy", "/x/skills/deploy/SKILL.md"),
        );
        let out = resolve_skill_prompt_entries(&prompt, &mut m, None, "/x");
        assert!(!out.prompt.contains("<available_skills>"), "emptied block removed:\n{}", out.prompt);
        assert!(out.prompt.contains("before") && out.prompt.contains("after"));
        // Enforcement entry still tracked.
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.entries[0].state, PermissionState::Ask);
    }

    #[test]
    fn prunes_structured_backtick_reference_to_hidden_skill() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(
            dir.path(),
            r#"{ "skills": { "deploy": "allow", "secret": "deny" } }"#,
        );
        let prompt = format!(
            "<available_skills>\n{}\n{}\n</available_skills>\n\nSkills table:\n- `deploy` deploys things\n- `secret` leaks things\n",
            skill_block("deploy", "/x/skills/deploy/SKILL.md"),
            skill_block("secret", "/x/skills/secret/SKILL.md"),
        );
        let out = resolve_skill_prompt_entries(&prompt, &mut m, None, "/x");
        assert!(out.prompt.contains("`deploy`"), "allowed skill row kept:\n{}", out.prompt);
        assert!(
            !out.prompt.contains("`secret`"),
            "the structured list item referencing the hidden skill must be pruned:\n{}",
            out.prompt
        );
    }

    #[test]
    fn no_available_skills_block_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manager_with_global(dir.path(), "{}");
        let prompt = "a system prompt with no skills block".to_string();
        let out = resolve_skill_prompt_entries(&prompt, &mut m, None, "/x");
        assert_eq!(out.prompt, prompt);
        assert!(out.entries.is_empty());
    }
}
