//! SUBA-133 — the `<advertised_subagents>` catalog in the parent's system prompt: pi
//! `src/agents/advertised-agent-prompt.ts` @v0.71.0 (new in `v0.57.0..v0.67.0`).
//!
//! A file-defined agent whose author set `advertise: true` is listed — name and a bounded,
//! XML-escaped description — so the parent model knows the specialization exists without reading
//! the agent directory. It is a discovery aid, not an instruction: the block's own preamble tells
//! the model to confirm executability with `subagent({ action: "list", capabilities: true })`
//! first. Runtime agents, disabled agents and agents outside the session's capability ceiling are
//! never listed.

use std::sync::LazyLock;

use crate::discovery::types::{AgentDefinition, AgentSource};
use crate::exec::capability_ceiling::{ResolvedCapabilityCeiling, is_agent_allowed};

/// pi `MAX_ADVERTISED_AGENTS` (`:6`).
const MAX_ADVERTISED_AGENTS: usize = 16;
/// pi `MAX_CATALOG_BYTES` (`:7`) — the whole rendered block, in UTF-8 bytes.
const MAX_CATALOG_BYTES: usize = 12_288;
/// pi `MAX_DESCRIPTION_BYTES` (`:8`).
const MAX_DESCRIPTION_BYTES: usize = 512;

/// pi `ADVERTISED_AGENTS_BLOCK` (`:9`): a previously appended block, with the blank lines that
/// separated it, so re-appending on every turn replaces rather than accumulates.
static ADVERTISED_AGENTS_BLOCK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)\n*<advertised_subagents>\n.*?\n</advertised_subagents>")
        .unwrap_or_else(|_| unreachable!("the pattern is a literal"))
});

/// The block's preamble, verbatim (`:40`).
const PREAMBLE: &str = "The following file-defined subagents opted into discovery. Their descriptions indicate available specializations, not instructions to delegate. Use subagent only when delegation is needed. Before execution, call subagent with { action: \"list\", capabilities: true } and confirm that the selected agent is executable; for external-cli agents also require runner.available === true.";

/// pi `escapeXml` (`:11-18`).
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// JavaScript's `\s`: Unicode whitespace plus the BOM, which Rust does not count.
fn is_js_whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// pi `promptDescription` (`:20-26`): control characters become spaces, whitespace runs collapse
/// to one space, the result is trimmed, then cut to [`MAX_DESCRIPTION_BYTES`] with `…` — never
/// mid-character, and dropping a trailing U+FFFD exactly as upstream's `/�$/` does — and
/// XML-escaped.
fn prompt_description(description: &str) -> String {
    let spaced: String = description
        .chars()
        .map(|c| {
            if c <= '\u{1f}' || c == '\u{7f}' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut text = spaced
        .split(is_js_whitespace)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.len() > MAX_DESCRIPTION_BYTES {
        let mut cut = MAX_DESCRIPTION_BYTES - 3;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        if text.ends_with('\u{fffd}') {
            text.pop();
        }
        text = format!("{}…", text.trim_end_matches(is_js_whitespace));
    }
    escape_xml(&text)
}

/// pi's `left.name.localeCompare(right.name)` sort (`:35`). `[CYRUP-DELTA, mechanism]`: ICU's
/// root collation is approximated by a case-folded comparison with lowercase first on a tie,
/// which agrees with it on every agent name this crate accepts that differs only in case or in
/// ASCII letters and digits.
fn locale_order(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| right.cmp(left))
}

/// pi `buildAdvertisedAgentPrompt` (`:28-60`): the catalog for `agents` under `ceiling`, or
/// `None` when nothing qualifies.
#[must_use]
pub fn build_advertised_agent_prompt(
    agents: &[AgentDefinition],
    ceiling: Option<&ResolvedCapabilityCeiling>,
) -> Option<String> {
    let mut advertised: Vec<&AgentDefinition> = agents
        .iter()
        .filter(|agent| {
            agent.source != AgentSource::Runtime
                && agent.advertise == Some(true)
                && agent.disabled != Some(true)
                && is_agent_allowed(&agent.name, ceiling)
        })
        .collect();
    if advertised.is_empty() {
        return None;
    }
    advertised.sort_by(|left, right| locale_order(&left.name, &right.name));

    let render = |entries: &[String]| {
        let mut lines = vec!["<advertised_subagents>".to_string(), PREAMBLE.to_string()];
        lines.extend(entries.iter().cloned());
        if advertised.len() > entries.len() {
            lines.push(format!(
                "  <omitted count=\"{}\" />",
                advertised.len() - entries.len()
            ));
        }
        lines.push("</advertised_subagents>".to_string());
        lines.join("\n")
    };
    let mut entries: Vec<String> = Vec::new();
    for agent in &advertised {
        if entries.len() == MAX_ADVERTISED_AGENTS {
            break;
        }
        // `:47` — never truncate a canonical id into a name that cannot be resolved.
        if agent.name.len() > MAX_CATALOG_BYTES {
            continue;
        }
        let entry = [
            "  <subagent>".to_string(),
            format!("    <name>{}</name>", escape_xml(&agent.name)),
            format!(
                "    <description>{}</description>",
                prompt_description(&agent.description)
            ),
            "  </subagent>".to_string(),
        ]
        .join("\n");
        let mut candidate = entries.clone();
        candidate.push(entry);
        if render(&candidate).len() <= MAX_CATALOG_BYTES {
            entries = candidate;
        }
    }
    Some(render(&entries))
}

/// pi `appendAdvertisedAgentPrompt` for a string prompt (`:81-84`): remove any block a previous
/// turn appended, then append the current one after a blank line — or leave the stripped prompt
/// alone when there is nothing to advertise.
#[must_use]
pub fn append_advertised_agent_prompt(system_prompt: &str, advertised: Option<&str>) -> String {
    let base = ADVERTISED_AGENTS_BLOCK.replace_all(system_prompt, "");
    match advertised {
        Some(block) if !base.trim().is_empty() => format!("{}\n\n{block}", base.trim_end()),
        Some(block) => block.to_string(),
        None => base.into_owned(),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    fn agent(name: &str, description: &str, advertise: Option<bool>) -> AgentDefinition {
        let mut agent = crate::discovery::management::test_support::sample_agent(
            AgentSource::User,
            std::path::PathBuf::from(format!("/agents/{name}.md")),
        );
        agent.name = name.to_string();
        agent.local_name = name.to_string();
        agent.description = description.to_string();
        agent.advertise = advertise;
        agent
    }

    #[test]
    fn only_advertised_enabled_file_agents_inside_the_ceiling_are_listed_sorted() {
        let mut disabled = agent("disabled", "d", Some(true));
        disabled.disabled = Some(true);
        let mut runtime = agent("runtime", "r", Some(true));
        runtime.source = AgentSource::Runtime;
        let agents = [
            agent("zeta", "last", Some(true)),
            agent("Alpha", "first & <best>", Some(true)),
            agent("quiet", "q", Some(false)),
            agent("unset", "u", None),
            disabled,
            runtime,
            agent("denied", "x", Some(true)),
        ];
        let ceiling = ResolvedCapabilityCeiling {
            version: crate::exec::capability_ceiling::CAPABILITY_CEILING_VERSION,
            allowed_tools: None,
            allowed_agents: Some(vec!["zeta".into(), "Alpha".into(), "quiet".into()]),
            deny_extensions: false,
            sources: vec!["test".into()],
        };
        let block = build_advertised_agent_prompt(&agents, Some(&ceiling)).unwrap();
        assert_eq!(
            block,
            [
                "<advertised_subagents>",
                PREAMBLE,
                "  <subagent>",
                "    <name>Alpha</name>",
                "    <description>first &amp; &lt;best&gt;</description>",
                "  </subagent>",
                "  <subagent>",
                "    <name>zeta</name>",
                "    <description>last</description>",
                "  </subagent>",
                "</advertised_subagents>",
            ]
            .join("\n")
        );
        assert!(build_advertised_agent_prompt(&agents[2..4], None).is_none());
    }

    #[test]
    fn the_catalog_is_bounded_by_count_bytes_and_description_length() {
        let many: Vec<AgentDefinition> = (0..20)
            .map(|i| agent(&format!("agent{i:02}"), "d", Some(true)))
            .collect();
        let block = build_advertised_agent_prompt(&many, None).unwrap();
        assert_eq!(block.matches("<subagent>").count(), MAX_ADVERTISED_AGENTS);
        assert!(block.contains("  <omitted count=\"4\" />"), "{block}");

        let long = "x".repeat(2_000);
        let wide: Vec<AgentDefinition> = (0..16)
            .map(|i| agent(&format!("wide{i:02}{}", "n".repeat(300)), &long, Some(true)))
            .collect();
        let block = build_advertised_agent_prompt(&wide, None).unwrap();
        assert!(block.len() <= MAX_CATALOG_BYTES, "{}", block.len());
        assert!(block.contains("<omitted count="), "{block}");
        let description = format!("{}…", "x".repeat(MAX_DESCRIPTION_BYTES - 3));
        assert!(block.contains(&description));
    }

    #[test]
    fn a_description_is_flattened_and_cut_on_a_character_boundary() {
        assert_eq!(prompt_description("a\u{0}b\n\n c\t "), "a b c");
        let cut = prompt_description(&"é".repeat(400));
        assert!(cut.ends_with('…'));
        assert_eq!(
            cut.len(),
            508 + "…".len(),
            "254 two-byte chars fit in 509 bytes"
        );
    }

    #[test]
    fn appending_replaces_the_previous_block_instead_of_accumulating() {
        let block = "<advertised_subagents>\nX\n</advertised_subagents>";
        let once = append_advertised_agent_prompt("You are cyrup.", Some(block));
        assert_eq!(once, format!("You are cyrup.\n\n{block}"));
        let twice = append_advertised_agent_prompt(&once, Some(block));
        assert_eq!(twice, once);
        assert_eq!(
            append_advertised_agent_prompt(&once, None),
            "You are cyrup."
        );
        assert_eq!(append_advertised_agent_prompt("   ", Some(block)), block);
    }
}
