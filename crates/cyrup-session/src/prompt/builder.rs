//! Pure system-prompt assembly (arch-06 §3.3/§6.1, R-06-001..005/012/017).
//!
//! [`SystemPromptBuilder::build_sections`] is pi's `buildSystemPromptSections`
//! (`packages/coding-agent/src/core/system-prompt.ts:121-180` @v1.0.0): the prompt as ORDERED, NAMED
//! sections — an untagged `preamble`, then `tools`, `rules`, `docs`, `addendum`, `project_context`,
//! `skills` and `cwd`, each wrapped in a tag of its own name — which is the form the transcript
//! stores (`SystemMessage.sections`) and diffs ([`diff_system_prompt_sections`]).
//! [`SystemPromptBuilder::build`] is pi's `buildSystemPrompt`: the same sections rendered through
//! `getSystemMessageText`, so the text a caller reads is the text the model is sent.
//!
//! Pure: no I/O, no clock, no panics. The inputs are the caller-assembled [`PromptInputs`] (already
//! trust-gated + precedence-resolved upstream).

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::Sections;
use cyrup_resources::SkillPointer;

use super::context_files::ContextFile;
use super::sections::{PREAMBLE, render_sections};
use super::skills_inject::skills_section_text;
use super::tool_prompts::ToolPromptContribution;

/// Docs-pointer paths for the progressive-disclosure section (DI-4). A `None` field omits its line.
#[derive(Clone, Debug, Default)]
pub struct DocsPointers {
    pub readme: Option<PathBuf>,
    pub docs: Option<PathBuf>,
    pub examples: Option<PathBuf>,
}

impl DocsPointers {
    fn is_empty(&self) -> bool {
        self.readme.is_none() && self.docs.is_none() && self.examples.is_none()
    }
}

/// Everything the pure builder needs. Assembled by the caller from cached + per-run pieces.
#[derive(Clone, Debug)]
pub struct PromptInputs {
    /// Resolved override: `None` => build default body; `Some` => replace body (R-06-003).
    pub custom_prompt: Option<Arc<str>>,
    /// Tools currently enabled (active set; may change at runtime — R-06-013).
    ///
    /// `None` = UNSET, which Pi resolves to its four-tool default (`system-prompt.ts:58`
    /// `const tools = selectedTools || ["read","bash","edit","write"]`). `Some(vec![])` is an
    /// EXPLICITLY EMPTY set and is NOT the default: an empty array is truthy in JS, so pi's `tools`
    /// stays `[]`, every `hasBash`/`hasGrep`/`hasFind`/`hasLs` (`:95-99`) is false and
    /// the skills gate at `:155` skips — and the custom-prompt branch does the same at `:64`
    /// (`!selectedTools || selectedTools.includes("read")`). Collapsing the two into one empty
    /// `Vec` advertised skills and tool guidelines to a caller that deliberately restricted the
    /// agent to zero tools.
    pub selected_tools: Option<Vec<Arc<str>>>,
    /// Selected tools whose declarations requests leave out (`prepareLoadout`'s hidden
    /// declarations; pi `BuildSystemPromptOptions.hiddenTools`, `system-prompt.ts:16-20` @v1.0.4).
    /// They are reachable only through another tool, so the tool list and the rules leave them out
    /// too, and the skills hint names none of them (CODE-020).
    pub hidden_tools: Vec<Arc<str>>,
    /// Per-tool one-line snippets + guideline bullets (R-06-012/013).
    pub tool_contributions: Vec<ToolPromptContribution>,
    /// Extra free-floating guideline bullets (non-tool-specific).
    pub prompt_guidelines: Vec<Arc<str>>,
    /// Append text: all append sources pre-joined in precedence order (R-06-004).
    pub append_system_prompt: Option<Arc<str>>,
    /// Extension-supplied sections, keyed by tag name, in insertion order (pi
    /// `BuildSystemPromptOptions.sections`, *"Additional XML-wrapped prompt sections keyed by tag
    /// name"*, `system-prompt.ts:29-30` @v1.1.0; EXT-084). A section whose name is one the builder
    /// already writes REPLACES that section where it stands; any other is appended after `cwd`;
    /// an empty one is skipped. The name check (`^[a-z][a-z0-9_-]*$`, never `preamble`) is
    /// [`super::SystemPromptOptions::validate`]'s, which the extension path runs before it gets
    /// here; the builder skips a name that fails it rather than panic.
    pub sections: Vec<(Arc<str>, Arc<str>)>,
    /// Working directory (footer + path normalization).
    pub cwd: PathBuf,
    /// Pre-loaded, trust-gated context files in final concat order (R-06-007/009).
    pub context_files: Arc<[ContextFile]>,
    /// Available skill pointers (already loaded/filtered; R-06-010/011).
    pub skills: Arc<[SkillPointer]>,
    /// Docs-pointer paths (DI-4).
    pub docs: DocsPointers,
    /// Injected for determinism/testability instead of `Date::now()`.
    pub today: time::Date,
}

impl Default for PromptInputs {
    fn default() -> Self {
        Self {
            custom_prompt: None,
            selected_tools: None,
            hidden_tools: Vec::new(),
            tool_contributions: Vec::new(),
            prompt_guidelines: Vec::new(),
            append_system_prompt: None,
            sections: Vec::new(),
            cwd: PathBuf::new(),
            context_files: Arc::from(Vec::new()),
            skills: Arc::from(Vec::new()),
            docs: DocsPointers::default(),
            today: time::Date::MIN,
        }
    }
}

/// Immutable `'static` template parts.
struct PromptTemplate {
    /// The `preamble` of the default prompt. [CYRUP-DELTA]: the product name (pi:
    /// `system-prompt.ts:146-147`, "You are an expert coding assistant operating inside pi, a coding
    /// agent harness. You help users by reading files, executing commands, editing code, and writing
    /// new files.").
    identity: &'static str,
    /// What the `tools` section lists when no selected tool has a snippet (`:149`).
    tools_empty: &'static str,
    /// The closing sentence of the `tools` section (`:151`).
    tools_extra: &'static str,
    baseline_guidelines: &'static [&'static str],
    /// Pi `system-prompt.ts:101-109` — a THREE-way branch over `hasBash`/`hasPowerShell`, not one
    /// string. Whichever shell tools are selected, the bullet names them.
    bash_fallback_guideline: &'static str,
    powershell_fallback_guideline: &'static str,
    bash_or_powershell_fallback_guideline: &'static str,
    docs_header: &'static str,
    docs_guidance: &'static [&'static str],
    /// The first line of the `project_context` section (`renderProjectContext`, `:94-97`).
    project_context_header: &'static str,
}

static DEFAULT_TEMPLATE: PromptTemplate = PromptTemplate {
    // [CYRUP-DELTA] identity references cyrup (was "pi").
    identity: "You are a coding assistant operating inside cyrup, helping with software \
               engineering tasks.",
    tools_empty: "(none)",
    tools_extra: "In addition to the tools above, you may have access to other custom tools \
                  depending on the project.",
    baseline_guidelines: &[
        "Be concise in your responses",
        "Show file paths clearly when working with files",
    ],
    bash_fallback_guideline: "Use bash for file operations like ls, rg, find",
    powershell_fallback_guideline: "Use PowerShell for file operations like listing, searching, and finding files",
    bash_or_powershell_fallback_guideline: "Use bash or PowerShell for file operations like listing, searching, and finding files",
    // [CYRUP-DELTA] docs pointer references cyrup docs (Pi `system-prompt.ts:153`, "Pi
    // documentation (read only when the user asks about pi itself, its SDK, extensions, themes,
    // skills, or TUI):").
    docs_header: "cyrup documentation (read only when the user asks about cyrup itself, its SDK, \
         extensions, themes, skills, or TUI):",
    // [CYRUP-DELTA] product name only; the instructions are Pi's `:157`, `:159`, `:160` verbatim.
    // Pi's `:158` ("When asked about: extensions (docs/extensions.md, …)") is cut down to the one
    // entry cyrup ships at that path: the other documentation files it lists are not at those paths.
    // The entry is pi's own wording for it (`system-prompt.ts:158` @v1.0.4).
    docs_guidance: &[
        "When reading cyrup docs or examples, resolve docs/... under Additional docs and \
         examples/... under Examples, not the current working directory",
        "When asked about: codemode scripts and non-LLM models such as classifiers and image \
         models (docs/codemode.md)",
        "When working on cyrup topics, read the docs and examples, and follow .md \
         cross-references before implementing",
        "Always read cyrup .md files completely and follow links to related docs (e.g., tui.md \
         for TUI API details)",
    ],
    project_context_header: "Project-specific instructions and guidelines:",
};

/// Stateless, pure assembler holding only the immutable template.
#[derive(Clone, Copy)]
pub struct SystemPromptBuilder {
    tmpl: &'static PromptTemplate,
}

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemPromptBuilder {
    pub fn new() -> Self {
        Self {
            tmpl: &DEFAULT_TEMPLATE,
        }
    }

    /// The prompt text the model reads (R-06-001..004): pi's `buildSystemPrompt`
    /// (`system-prompt.ts:195-197` @v1.0.0), the structured sections of
    /// [`Self::build_sections`] rendered exactly as the transcript's system message replays them —
    /// every section's text, joined by a blank line. Pure: no I/O, no clock, no panics.
    pub fn build(&self, inp: &PromptInputs) -> String {
        render_sections(&self.build_sections(inp))
    }

    /// The ordered, independently replaceable sections of the structured system prompt: pi's
    /// `buildSystemPromptSections` (`system-prompt.ts:121-180` @v1.0.0).
    ///
    /// `preamble` is untagged text; every other section is `<name>\n…\n</name>`, so the model can
    /// match a later update to it. The order is the order pi inserts them in, and it is observable —
    /// it is the order the model reads them in.
    ///
    /// Extension-supplied sections ([`PromptInputs::sections`]) are applied last, as pi's
    /// `for (const [name, content] of Object.entries(customSections)) if (content)
    /// promptSections[name] = content;` (`system-prompt.ts:185-187` @v1.1.0): assigning an existing
    /// key keeps its position in a JS object, so a custom `rules` replaces the built rules in place
    /// and a new name lands after `cwd`.
    pub fn build_sections(&self, inp: &PromptInputs) -> Sections {
        let t = self.tmpl;
        let mut parts: Vec<(std::borrow::Cow<'static, str>, String)> = Vec::with_capacity(8);

        // SESS-059 — Pi gates the skills section on a tool that can READ a skill file being in the
        // effective set, `read` first, then `bash`. CODE-020 (`c30840c2e` @v1.0.4): the reader is
        // looked for among the DECLARED tools first; a reader that is selected but hidden is still
        // reachable through another tool, so the skills stay and the hint names no tool
        // (`"indirect"`):
        // `readers.find((tool) => declaredTools.includes(tool)) ?? (readers.some((tool) =>
        // selectedTools.includes(tool)) ? "indirect" : undefined)` (`:175-178`).
        let skill_file_read_tool = ["read", "bash"]
            .into_iter()
            .find(|tool| is_declared(inp, tool))
            .or_else(|| {
                ["read", "bash"]
                    .into_iter()
                    .any(|tool| is_selected(inp.selected_tools.as_ref(), tool))
                    .then_some(INDIRECT_READER)
            });

        // `if (customPrompt)` is a truthiness test: the empty string is NOT a custom prompt.
        match inp.custom_prompt.as_deref().filter(|c| !c.is_empty()) {
            // ── FULL REPLACEMENT of the preamble (R-06-003); `tools`, `rules` and `docs` are the
            // default prompt's and do not appear.
            Some(custom) => parts.push((PREAMBLE.into(), custom.to_owned())),
            None => {
                parts.push((PREAMBLE.into(), t.identity.to_owned()));
                parts.push(("tools".into(), self.tools_section(inp)));
                parts.push(("rules".into(), self.rules_section(inp)));
                if let Some(docs) = docs_section(t, &inp.docs) {
                    parts.push(("docs".into(), docs));
                }
            }
        }

        // ── SHARED TAIL (runs for BOTH custom + default — R-06-003 mandates it) ──
        // 5. append (`if (appendSystemPrompt)`: truthy, so only the empty string is skipped)
        if let Some(a) = inp
            .append_system_prompt
            .as_deref()
            .filter(|a| !a.is_empty())
        {
            parts.push(("addendum".into(), a.to_owned()));
        }
        // 6. project context files (already trust-gated + ordered)
        if !inp.context_files.is_empty() {
            parts.push((
                "project_context".into(),
                project_context_text(t, &inp.context_files),
            ));
        }
        // 7. skills (only if `read` or `bash` can load them — R-06-010, SESS-059)
        if let Some(tool) = skill_file_read_tool
            && let Some(skills) = skills_section_text(&inp.skills, tool)
        {
            parts.push(("skills".into(), skills));
        }
        // 8. cwd
        parts.push(("cwd".into(), normalize_slashes(&inp.cwd)));
        // 9. extension sections (EXT-084)
        for (name, content) in &inp.sections {
            if content.is_empty() || !super::options::is_valid_section_name(name) {
                continue;
            }
            match parts.iter_mut().find(|(n, _)| **n == **name) {
                Some(slot) => slot.1 = content.to_string(),
                None => parts.push((name.to_string().into(), content.to_string())),
            }
        }

        parts
            .into_iter()
            .map(|(name, content)| {
                let text = if name == PREAMBLE {
                    content
                } else {
                    format!("<{name}>\n{content}\n</{name}>")
                };
                (name, Some(text))
            })
            .collect()
    }

    /// `promptSections.tools` (`:148-151`): the selected tools that HAVE a snippet, in selection
    /// order, then the closing sentence (R-06-012).
    fn tools_section(&self, inp: &PromptInputs) -> String {
        let t = self.tmpl;
        let mut listing = String::new();
        for name in declared_names(inp) {
            let snippet = inp
                .tool_contributions
                .iter()
                .find(|c| &*c.tool == name)
                .and_then(|c| c.snippet.as_deref());
            if let Some(snippet) = snippet {
                if !listing.is_empty() {
                    listing.push('\n');
                }
                listing.push_str("- ");
                listing.push_str(name);
                listing.push_str(": ");
                listing.push_str(snippet);
            }
        }
        if listing.is_empty() {
            listing.push_str(t.tools_empty);
        }
        format!("{listing}\n\n{}", t.tools_extra)
    }

    /// `promptSections.rules` (`buildRules`, `:81-118`): deduplicated, trimmed bullets in insertion
    /// order.
    fn rules_section(&self, inp: &PromptInputs) -> String {
        let t = self.tmpl;
        let mut rules: Vec<String> = Vec::new();
        // 3a. conditional file-exploration fallback (Pi `system-prompt.ts:101-109`). The gate is
        // `(hasBash || hasPowerShell)`, and the bullet names whichever shells are actually selected
        // — a PowerShell-only session must not be told to use `ls, rg, find`.
        let has = |n: &str| is_declared(inp, n);
        let has_bash = has("bash");
        let has_powershell = has("powershell");
        if (has_bash || has_powershell) && !has("grep") && !has("find") && !has("ls") {
            let guideline = if has_bash && has_powershell {
                t.bash_or_powershell_fallback_guideline
            } else if has_powershell {
                t.powershell_fallback_guideline
            } else {
                t.bash_fallback_guideline
            };
            push_rule(&mut rules, guideline);
        }
        // 3b. tool-specific guidelines, in selection order (named per func-03 R-03-039), of the
        // DECLARED tools: a hidden tool's guidelines are shown with its declaration where the model
        // meets it (codemode), not as rules for a tool the request does not carry (CODE-020).
        for name in declared_names(inp) {
            if let Some(c) = inp.tool_contributions.iter().find(|c| &*c.tool == name) {
                for g in &c.guidelines {
                    push_rule(&mut rules, g);
                }
            }
        }
        // 3c. caller-supplied extra guidelines
        for g in &inp.prompt_guidelines {
            push_rule(&mut rules, g);
        }
        // 3d. baseline (always)
        for g in t.baseline_guidelines {
            push_rule(&mut rules, g);
        }
        rules
            .iter()
            .map(|rule| format!("- {rule}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Cheap, non-cryptographic fingerprint of the output-affecting inputs (R-06-017).
    ///
    /// **Currently unused in production.** `rg -n inputs_fingerprint crates/` finds only this
    /// definition and the unit test; nothing caches `(fingerprint -> prompt)`. Pi has no prompt
    /// fingerprint at all — `buildSystemPrompt` is a pure function rebuilt explicitly by
    /// `_rebuildSystemPrompt` (`agent-session.ts:1021`) — so there is no upstream contract to
    /// inherit. An earlier doc here asserted "the agent caches `(fingerprint -> prompt)` for the
    /// session and only rebuilds when it changes", which was never true.
    ///
    /// Every output-affecting field of [`PromptInputs`] must be hashed here or a future cache
    /// would serve a stale prompt. That explicitly includes
    /// [`cyrup_resources::SkillPointer::disable_model_invocation`], which
    /// [`super::skills_inject::emit_skills_section`] uses to drop a skill from the prompt entirely.
    pub fn inputs_fingerprint(&self, inp: &PromptInputs) -> u64 {
        let mut h = rustc_hash::FxHasher::default();
        opt_str_hash(&mut h, &inp.custom_prompt);
        // UNSET (Pi's `||` default) and an explicitly EMPTY set produce different prompts, so the
        // discriminant is hashed before the members.
        match &inp.selected_tools {
            None => 0u8.hash(&mut h),
            Some(v) => {
                1u8.hash(&mut h);
                // Order is part of identity for tool listing, but membership is what gates; hash a
                // sorted view so a mere reorder doesn't force a rebuild.
                let mut tools: Vec<&str> = v.iter().map(|s| &**s).collect();
                tools.sort_unstable();
                tools.hash(&mut h);
            }
        }
        let mut hidden: Vec<&str> = inp.hidden_tools.iter().map(|t| &**t).collect();
        hidden.sort_unstable();
        hidden.hash(&mut h);
        for c in &inp.tool_contributions {
            c.tool.hash(&mut h);
            opt_str_hash(&mut h, &c.snippet);
            for g in &c.guidelines {
                g.hash(&mut h);
            }
        }
        for g in &inp.prompt_guidelines {
            g.hash(&mut h);
        }
        opt_str_hash(&mut h, &inp.append_system_prompt);
        inp.cwd.hash(&mut h);
        for cf in inp.context_files.iter() {
            cf.path.hash(&mut h);
            cf.content.hash(&mut h);
        }
        for s in inp.skills.iter() {
            s.name.hash(&mut h);
            s.description.hash(&mut h);
            s.path.hash(&mut h);
            // Output-affecting: a `disable-model-invocation: true` skill is filtered out of
            // `<available_skills>` entirely (`skills_inject.rs`, Pi `skills.ts:336`). Omitting it
            // meant flipping the frontmatter did not change the fingerprint.
            s.disable_model_invocation.hash(&mut h);
        }
        inp.docs.readme.hash(&mut h);
        inp.docs.docs.hash(&mut h);
        inp.docs.examples.hash(&mut h);
        // `today` is deliberately NOT hashed: with the stale `Current date:` footer removed
        // (see `emit_footer`) it affects no byte of the output, and hashing it forced a rebuild at
        // every midnight boundary.
        h.finish()
    }
}

fn opt_str_hash<H: Hasher>(h: &mut H, s: &Option<Arc<str>>) {
    match s {
        Some(v) => {
            1u8.hash(h);
            v.hash(h);
        }
        None => 0u8.hash(h),
    }
}

/// Pi's four-tool fallback for an UNSET selection (`system-prompt.ts:58`).
pub const DEFAULT_SELECTED_TOOLS: &[&str] = &["read", "bash", "edit", "write"];

/// A tool is selected if the set is unset (Pi's `||` default) or explicitly names it.
///
/// An explicitly EMPTY set selects nothing — including `read`, which is what gates the skills
/// section (`system-prompt.ts:165`).
fn is_selected(selected: Option<&Vec<Arc<str>>>, name: &str) -> bool {
    match selected {
        None => DEFAULT_SELECTED_TOOLS.contains(&name),
        Some(v) => v.iter().any(|t| &**t == name),
    }
}

/// The selected tool names, in selection order. UNSET resolves to pi's four-tool default
/// (`system-prompt.ts:58`); an explicitly empty set stays empty.
fn selected_names(selected: Option<&Vec<Arc<str>>>) -> Vec<&str> {
    match selected {
        None => DEFAULT_SELECTED_TOOLS.to_vec(),
        Some(v) => v.iter().map(|t| &**t).collect(),
    }
}

/// What the skills hint names when the reader is selected but hidden (pi `formatSkillsForPrompt`'s
/// third `fileReadTool`, `skills.ts` @v1.0.4): no tool.
pub(super) const INDIRECT_READER: &str = "indirect";

/// A tool is declared if it is selected and its declaration is not hidden: pi's `declaredTools =
/// selectedTools.filter((name) => !hiddenTools.includes(name))` (`system-prompt.ts:150` @v1.0.4).
fn is_declared(inp: &PromptInputs, name: &str) -> bool {
    is_selected(inp.selected_tools.as_ref(), name) && !inp.hidden_tools.iter().any(|t| &**t == name)
}

/// The declared tool names, in selection order.
fn declared_names(inp: &PromptInputs) -> Vec<&str> {
    selected_names(inp.selected_tools.as_ref())
        .into_iter()
        .filter(|name| !inp.hidden_tools.iter().any(|t| &**t == *name))
        .collect()
}

/// Pi's `addRule` (`system-prompt.ts:88-93`): trimmed, non-empty, first occurrence wins.
fn push_rule(rules: &mut Vec<String>, rule: &str) {
    let rule = rule.trim();
    if rule.is_empty() || rules.iter().any(|r| r == rule) {
        return;
    }
    rules.push(rule.to_owned());
}

/// Pi `system-prompt.ts:153-160`, ported line-for-line. The three trailing bullets are
/// BEHAVIOURAL — they are what makes the pointers usable:
/// * `:157` resolve `docs/…` under Additional docs and `examples/…` under Examples, not the cwd;
/// * `:159` read the docs and examples, and follow `.md` cross-references before implementing;
/// * `:160` read `.md` files completely and follow links to related docs.
///
/// Pi's block carries **no guard**, so every default prompt has it; cyrup's paths are `Option`s and
/// the section is left out when all three are absent. That guard is only reachable because the sole
/// production caller still passes `DocsPointers::default()` — see SESS-035; the path helpers
/// themselves belong in `cyrup-config` (Pi `config.ts:427-439`, three
/// `resolve(join(getPackageDir(), …))` calls).
fn docs_section(t: &PromptTemplate, docs: &DocsPointers) -> Option<String> {
    if docs.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = Vec::with_capacity(6);
    if let Some(p) = &docs.readme {
        lines.push(format!("- Main documentation: {}", p.to_string_lossy()));
    }
    if let Some(p) = &docs.docs {
        lines.push(format!("- Additional docs: {}", p.to_string_lossy()));
    }
    if let Some(p) = &docs.examples {
        lines.push(format!(
            "- Examples: {} (extensions, custom tools, SDK)",
            p.to_string_lossy()
        ));
    }
    lines.extend(t.docs_guidance.iter().map(|line| format!("- {line}")));
    Some(format!("{}\n{}", t.docs_header, lines.join("\n")))
}

/// Pi `renderProjectContext` (`system-prompt.ts:72-79`): the header, then one
/// `<project_instructions path="…">` block per file, joined by a blank line.
fn project_context_text(t: &PromptTemplate, files: &[ContextFile]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(files.len() + 1);
    parts.push(t.project_context_header.to_owned());
    for cf in files {
        // lossy: never panic on non-UTF8
        parts.push(format!(
            "<project_instructions path=\"{}\">\n{}\n</project_instructions>",
            cf.path.to_string_lossy(),
            cf.content
        ));
    }
    parts.join("\n\n")
}

/// Pi's `promptSections.cwd = cwd.replace(/\\/g, "/")` (`system-prompt.ts:170`).
fn normalize_slashes(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}
