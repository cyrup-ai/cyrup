//! The prompt's construction options as extensions see and edit them: pi's
//! `NormalizedBuildSystemPromptOptions` (`packages/coding-agent/src/core/system-prompt.ts:9-49`
//! @v1.1.0), EXT-084.
//!
//! pi hands every `before_agent_start` handler ONE mutable options object — *"Mutable prompt
//! sections. Later handlers observe mutations made by earlier handlers."* — and renders the prompt
//! from it on demand (`event.systemPrompt` is a getter over `buildSystemPrompt(currentOptions)`,
//! `core/extensions/runner.ts:1425-1448`). [`SystemPromptOptions`] is that object; its JSON is the
//! object's JSON, key for key and in pi's order, so a handler on either tier reads and returns the
//! shape upstream documents.
//!
//! [`SystemPromptOptions::normalize`] is `normalizeBuildSystemPromptOptions` (`:60-75`): every
//! collection present, `selectedTools` defaulting to pi's four tools. [`SystemPromptOptions::render`]
//! is `buildSystemPrompt` (`:208-210`) — the forced text when a handler forced one, otherwise the
//! structured sections rendered — and [`SystemPromptOptions::build_sections`] is
//! `buildSystemPromptSections` (`:128-193`), section-name check included.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::Sections;
use cyrup_resources::SkillPointer;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::builder::{DEFAULT_SELECTED_TOOLS, DocsPointers, PromptInputs, SystemPromptBuilder};
use super::context_files::{ContextFile, ContextScope};
use super::sections::{PREAMBLE, render_sections};
use super::tool_prompts::ToolPromptContribution;

/// One pre-loaded context file (pi `contextFiles: Array<{ path; content }>`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptContextFile {
    pub path: String,
    pub content: String,
}

/// One skill the prompt advertises (pi `Skill`, `core/skills.ts`), as far as the prompt reads it:
/// `name`, `description`, `filePath` and `disableModelInvocation`. Unknown keys an extension adds
/// are kept, so a skill passes through a handler unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSkill {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub disable_model_invocation: bool,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

/// pi's `NormalizedBuildSystemPromptOptions`. Field order is pi's, which is the JSON key order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemPromptOptions {
    /// Custom system prompt (replaces the default prefix).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_prompt: Option<String>,
    /// Exact full prompt replacement set by a `before_agent_start` handler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_system_prompt: Option<String>,
    pub selected_tools: Vec<String>,
    pub hidden_tools: Vec<String>,
    pub tool_snippets: IndexMap<String, String>,
    pub tool_guidelines: IndexMap<String, Vec<String>>,
    pub prompt_guidelines: Vec<String>,
    pub append_system_prompt: String,
    /// Additional XML-wrapped prompt sections keyed by tag name.
    pub sections: IndexMap<String, String>,
    pub cwd: String,
    pub context_files: Vec<PromptContextFile>,
    pub skills: Vec<PromptSkill>,
}

/// The un-normalized input (pi `BuildSystemPromptOptions`): every collection optional.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOptions {
    #[serde(default)]
    custom_prompt: Option<String>,
    #[serde(default)]
    force_system_prompt: Option<String>,
    #[serde(default)]
    selected_tools: Option<Vec<String>>,
    #[serde(default)]
    hidden_tools: Option<Vec<String>>,
    #[serde(default)]
    tool_snippets: Option<IndexMap<String, String>>,
    #[serde(default)]
    tool_guidelines: Option<IndexMap<String, Vec<String>>>,
    #[serde(default)]
    prompt_guidelines: Option<Vec<String>>,
    #[serde(default)]
    append_system_prompt: Option<String>,
    #[serde(default)]
    sections: Option<IndexMap<String, String>>,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    context_files: Option<Vec<PromptContextFile>>,
    #[serde(default)]
    skills: Option<Vec<PromptSkill>>,
}

/// pi's `Invalid system prompt section name: <name>` (`system-prompt.ts:141`): a custom section must
/// match `^[a-z][a-z0-9_-]*$` and may not be `preamble`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Invalid system prompt section name: {0}")]
pub struct InvalidSectionName(pub String);

/// pi `SYSTEM_PROMPT_SECTION_NAME` (`system-prompt.ts:58`) plus the `preamble` exclusion.
pub fn is_valid_section_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    first_ok
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && name != PREAMBLE
}

impl SystemPromptOptions {
    /// pi `normalizeBuildSystemPromptOptions` over a JSON options object. An object that is not
    /// one, or whose fields have the wrong types, is refused with serde's reason.
    pub fn normalize(raw: &Value) -> Result<Self, String> {
        let raw: RawOptions = serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
        Ok(Self {
            custom_prompt: raw.custom_prompt,
            force_system_prompt: raw.force_system_prompt,
            selected_tools: raw.selected_tools.unwrap_or_else(|| {
                DEFAULT_SELECTED_TOOLS
                    .iter()
                    .map(|t| (*t).to_string())
                    .collect()
            }),
            hidden_tools: raw.hidden_tools.unwrap_or_default(),
            tool_snippets: raw.tool_snippets.unwrap_or_default(),
            tool_guidelines: raw.tool_guidelines.unwrap_or_default(),
            prompt_guidelines: raw.prompt_guidelines.unwrap_or_default(),
            append_system_prompt: raw.append_system_prompt.unwrap_or_default(),
            sections: raw.sections.unwrap_or_default(),
            cwd: raw.cwd,
            context_files: raw.context_files.unwrap_or_default(),
            skills: raw.skills.unwrap_or_default(),
        })
    }

    /// The options as the JSON object an extension receives.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// The custom section names pi's `buildSystemPromptSections` refuses (`:136-140`).
    pub fn validate(&self) -> Result<(), InvalidSectionName> {
        match self.sections.keys().find(|n| !is_valid_section_name(n)) {
            Some(bad) => Err(InvalidSectionName(bad.clone())),
            None => Ok(()),
        }
    }

    /// The builder's inputs for these options. `docs` is the one input pi's options do not carry
    /// (pi reads its documentation paths from the package, `config.ts`), so the host supplies it.
    pub fn to_inputs(&self, docs: &DocsPointers) -> PromptInputs {
        let mut names: Vec<&String> = self.tool_snippets.keys().collect();
        for name in self.tool_guidelines.keys() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let tool_contributions = names
            .into_iter()
            .map(|name| ToolPromptContribution {
                tool: Arc::from(name.as_str()),
                snippet: self
                    .tool_snippets
                    .get(name)
                    .filter(|s| !s.is_empty())
                    .map(|s| Arc::from(s.as_str())),
                guidelines: self
                    .tool_guidelines
                    .get(name)
                    .map(|g| g.iter().map(|r| Arc::from(r.as_str())).collect())
                    .unwrap_or_default(),
            })
            .collect();
        PromptInputs {
            custom_prompt: self.custom_prompt.as_deref().map(Arc::from),
            selected_tools: Some(
                self.selected_tools
                    .iter()
                    .map(|t| Arc::from(t.as_str()))
                    .collect(),
            ),
            hidden_tools: self
                .hidden_tools
                .iter()
                .map(|t| Arc::from(t.as_str()))
                .collect(),
            tool_contributions,
            prompt_guidelines: self
                .prompt_guidelines
                .iter()
                .map(|g| Arc::from(g.as_str()))
                .collect(),
            append_system_prompt: (!self.append_system_prompt.is_empty())
                .then(|| Arc::from(self.append_system_prompt.as_str())),
            sections: self
                .sections
                .iter()
                .map(|(n, c)| (Arc::from(n.as_str()), Arc::from(c.as_str())))
                .collect(),
            cwd: PathBuf::from(&self.cwd),
            context_files: self
                .context_files
                .iter()
                .map(|f| ContextFile {
                    path: PathBuf::from(&f.path),
                    content: Arc::from(f.content.as_str()),
                    scope: ContextScope::Cwd,
                })
                .collect::<Vec<_>>()
                .into(),
            skills: self
                .skills
                .iter()
                .map(|s| SkillPointer {
                    name: s.name.clone(),
                    description: s.description.clone(),
                    path: PathBuf::from(&s.file_path),
                    disable_model_invocation: s.disable_model_invocation,
                })
                .collect::<Vec<_>>()
                .into(),
            docs: docs.clone(),
            ..PromptInputs::default()
        }
    }

    /// pi `buildSystemPromptSections` (`system-prompt.ts:128-193`): the structured sections, which
    /// is what the transcript stores even when a forced prompt is in force.
    pub fn build_sections(&self, docs: &DocsPointers) -> Result<Sections, InvalidSectionName> {
        self.validate()?;
        Ok(SystemPromptBuilder::new().build_sections(&self.to_inputs(docs)))
    }

    /// pi `buildSystemPrompt` (`system-prompt.ts:199-210`): the forced prompt when one is set,
    /// otherwise the sections rendered.
    pub fn render(&self, docs: &DocsPointers) -> Result<String, InvalidSectionName> {
        if let Some(forced) = &self.force_system_prompt {
            return Ok(forced.clone());
        }
        Ok(render_sections(&self.build_sections(docs)?))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts(v: Value) -> SystemPromptOptions {
        SystemPromptOptions::normalize(&v).expect("normalizes")
    }

    fn names(sections: &Sections) -> Vec<String> {
        sections.iter().map(|(n, _)| n.to_string()).collect()
    }

    /// pi `normalizeBuildSystemPromptOptions` (`system-prompt.ts:60-75` @v1.1.0): every collection
    /// present, `selectedTools` defaulting to the four tools, and the object's keys in pi's order —
    /// `undefined` `customPrompt`/`forceSystemPrompt` dropped by `JSON.stringify`.
    #[test]
    fn normalization_fills_every_collection_in_pis_key_order() {
        let o = opts(json!({ "cwd": "/w" }));
        assert_eq!(o.selected_tools, ["read", "bash", "edit", "write"]);
        let keys: Vec<String> = o.to_value().as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            [
                "selectedTools",
                "hiddenTools",
                "toolSnippets",
                "toolGuidelines",
                "promptGuidelines",
                "appendSystemPrompt",
                "sections",
                "cwd",
                "contextFiles",
                "skills",
            ]
        );
        // An explicit empty selection stays empty: `[]` is not `undefined`.
        assert!(
            opts(json!({ "cwd": "/w", "selectedTools": [] }))
                .selected_tools
                .is_empty()
        );
        // A value round-trips through a handler unchanged.
        assert_eq!(opts(o.to_value()), o);
        assert!(SystemPromptOptions::normalize(&json!({ "selectedTools": "read" })).is_err());
    }

    /// pi `buildSystemPromptSections` (`:185-192`): a custom section is applied last — a new name
    /// after `cwd`, an existing one replaced where it stands, an empty one skipped — and wrapped in a
    /// tag of its own name.
    #[test]
    fn custom_sections_land_after_cwd_or_replace_in_place() {
        let o = opts(json!({
            "cwd": "/w",
            "sections": { "house_rules": "No tabs.", "rules": "- Only this rule", "empty": "" },
        }));
        let sections = o.build_sections(&DocsPointers::default()).unwrap();
        assert_eq!(
            names(&sections),
            ["preamble", "tools", "rules", "cwd", "house_rules"]
        );
        assert_eq!(
            sections.get("rules"),
            Some(Some("<rules>\n- Only this rule\n</rules>"))
        );
        assert_eq!(
            sections.get("house_rules"),
            Some(Some("<house_rules>\nNo tabs.\n</house_rules>"))
        );
    }

    /// pi's name check (`:136-140`) and its message.
    #[test]
    fn an_invalid_section_name_is_refused_with_pis_message() {
        for bad in ["preamble", "Rules", "1st", "has space", ""] {
            let o = opts(json!({ "cwd": "/w", "sections": { bad: "x" } }));
            let err = o.build_sections(&DocsPointers::default()).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("Invalid system prompt section name: {bad}")
            );
        }
        assert!(is_valid_section_name("a-b_9"));
    }

    /// pi `buildSystemPrompt` (`:199-210`): a forced prompt is the whole text; without one the
    /// sections are rendered — and the sections themselves ignore the force, since the transcript
    /// keeps them whatever a handler forced.
    #[test]
    fn a_forced_prompt_is_the_rendering_but_not_the_sections() {
        let mut o = opts(json!({ "cwd": "/w" }));
        let plain = o.render(&DocsPointers::default()).unwrap();
        assert!(plain.contains("<cwd>\n/w\n</cwd>"), "{plain}");
        o.force_system_prompt = Some("FORCED".into());
        assert_eq!(o.render(&DocsPointers::default()).unwrap(), "FORCED");
        assert_eq!(
            render_sections(&o.build_sections(&DocsPointers::default()).unwrap()),
            plain
        );
    }

    /// The tool maps reach the builder as pi reads them: a snippet lists a DECLARED tool, its
    /// guidelines become rules, and a hidden tool shows in neither (`:146-158`).
    #[test]
    fn tool_snippets_and_guidelines_follow_the_declared_tools() {
        let o = opts(json!({
            "cwd": "/w",
            "selectedTools": ["read", "grep"],
            "hiddenTools": ["grep"],
            "toolSnippets": { "read": "Read a file", "grep": "Search", "write": "Write" },
            "toolGuidelines": { "read": ["Read before you edit"], "grep": ["Prefer grep"] },
        }));
        let s = o.build_sections(&DocsPointers::default()).unwrap();
        let tools = s.get("tools").flatten().unwrap();
        let rules = s.get("rules").flatten().unwrap();
        assert!(tools.contains("- read: Read a file"), "{tools}");
        assert!(
            !tools.contains("grep") && !tools.contains("write"),
            "{tools}"
        );
        assert!(rules.contains("- Read before you edit"), "{rules}");
        assert!(!rules.contains("Prefer grep"), "{rules}");
    }
}
