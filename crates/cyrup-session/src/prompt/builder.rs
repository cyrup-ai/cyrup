//! Pure system-prompt assembly (arch-06 §3.3/§6.1, R-06-001..005/012/017).
//!
//! [`SystemPromptBuilder::build`] is the single hot path: no I/O, no clock, no panics. It writes
//! into one pre-sized `String` from `&'static` template parts plus the caller-assembled
//! [`PromptInputs`] (already trust-gated + precedence-resolved upstream).

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_resources::SkillPointer;

use super::context_files::ContextFile;
use super::skills_inject::emit_skills_section;
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
    /// `None` = UNSET, which Pi resolves to its four-tool default (`system-prompt.ts:81`
    /// `const tools = selectedTools || ["read","bash","edit","write"]`). `Some(vec![])` is an
    /// EXPLICITLY EMPTY set and is NOT the default: an empty array is truthy in JS, so pi's `tools`
    /// stays `[]`, every `hasBash`/`hasGrep`/`hasFind`/`hasLs`/`hasRead` (`:97-101`) is false and
    /// the skills gate at `:155` skips — and the custom-prompt branch does the same at `:64`
    /// (`!selectedTools || selectedTools.includes("read")`). Collapsing the two into one empty
    /// `Vec` advertised skills and tool guidelines to a caller that deliberately restricted the
    /// agent to zero tools.
    pub selected_tools: Option<Vec<Arc<str>>>,
    /// Per-tool one-line snippets + guideline bullets (R-06-012/013).
    pub tool_contributions: Vec<ToolPromptContribution>,
    /// Extra free-floating guideline bullets (non-tool-specific).
    pub prompt_guidelines: Vec<Arc<str>>,
    /// Append text: all append sources pre-joined in precedence order (R-06-004).
    pub append_system_prompt: Option<Arc<str>>,
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
            tool_contributions: Vec::new(),
            prompt_guidelines: Vec::new(),
            append_system_prompt: None,
            cwd: PathBuf::new(),
            context_files: Arc::from(Vec::new()),
            skills: Arc::from(Vec::new()),
            docs: DocsPointers::default(),
            today: time::Date::MIN,
        }
    }
}

/// Immutable `'static` template parts (DI-1: only the final `String` is allocated per build).
struct PromptTemplate {
    identity: &'static str,
    tools_header: &'static str,
    tools_empty: &'static str,
    tools_extra: &'static str,
    guidelines_header: &'static str,
    baseline_guidelines: &'static [&'static str],
    bash_fallback_guideline: &'static str,
    docs_header: &'static str,
    docs_guidance: &'static [&'static str],
    project_context_open: &'static str,
    project_context_close: &'static str,
}

static DEFAULT_TEMPLATE: PromptTemplate = PromptTemplate {
    // [CYRUP-DELTA] identity references cyrup (was "pi").
    identity: "You are a coding assistant operating inside cyrup, helping with software \
               engineering tasks.",
    tools_header: "Available tools:",
    tools_empty: "(none)",
    tools_extra: "In addition to the tools above, you may have access to custom tools provided by \
                  extensions and skills.",
    guidelines_header: "Guidelines:",
    baseline_guidelines: &[
        "Be concise in your responses",
        "Show file paths clearly when working with files",
    ],
    bash_fallback_guideline: "Use bash for file operations like ls, rg, find",
    // [CYRUP-DELTA] docs pointer references cyrup docs (Pi `system-prompt.ts:131`, "Pi
    // documentation (read only when the user asks about pi itself, its SDK, extensions, themes,
    // skills, or TUI):").
    docs_header:
        "cyrup documentation (read only when the user asks about cyrup itself, its SDK, \
         extensions, themes, skills, or TUI):",
    // [CYRUP-DELTA] product name only; the instructions are Pi's `:135`, `:137`, `:138` verbatim.
    docs_guidance: &[
        "When reading cyrup docs or examples, resolve docs/... under Additional docs and \
         examples/... under Examples, not the current working directory",
        "When working on cyrup topics, read the docs and examples, and follow .md \
         cross-references before implementing",
        "Always read cyrup .md files completely and follow links to related docs (e.g., tui.md \
         for TUI API details)",
    ],
    // Byte-for-byte with Pi `system-prompt.ts:146-147` / `:151` (identical in the custom-prompt
    // branch at `:55-56` / `:60`): `"\n\n<project_context>\n\n"` + `"Project-specific instructions
    // and guidelines:\n\n"`, closed by `"</project_context>\n"` — note the TRAILING newline, which
    // is what separates the block from the `\nCurrent working directory:` footer.
    project_context_open: "<project_context>\n\nProject-specific instructions and guidelines:\n\n",
    project_context_close: "</project_context>\n",
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
        Self { tmpl: &DEFAULT_TEMPLATE }
    }

    /// Build the full prompt (R-06-001..004). Pure: no I/O, no clock, no panics.
    pub fn build(&self, inp: &PromptInputs) -> String {
        let t = self.tmpl;
        let mut out = String::with_capacity(2048);

        // Pi gates the skills section on `read` being in the effective set: `hasRead` for the
        // default body (`system-prompt.ts:101,155`) and `!selectedTools || selectedTools
        // .includes("read")` for the custom-prompt branch (`:64-65`) — the same predicate.
        let read_available = is_selected(inp.selected_tools.as_ref(), "read");

        if let Some(custom) = &inp.custom_prompt {
            // ── FULL REPLACEMENT (R-06-003) ──
            out.push_str(custom);
        } else {
            self.build_default_body(&mut out, inp);
        }

        // ── SHARED TAIL (runs for BOTH custom + default — R-06-003 mandates it) ──
        // 5. append
        if let Some(a) = &inp.append_system_prompt {
            let a = a.trim();
            if !a.is_empty() {
                out.push_str("\n\n");
                out.push_str(a);
            }
        }
        // 6. project context files (already trust-gated + ordered)
        emit_context_files(&mut out, t, &inp.context_files);
        // 7. skills (only if read available — R-06-010)
        if read_available {
            emit_skills_section(&mut out, &inp.skills);
        }
        // 8. footer
        emit_footer(&mut out, inp);
        out
    }

    fn build_default_body(&self, out: &mut String, inp: &PromptInputs) {
        let t = self.tmpl;
        // 1. identity
        out.push_str(t.identity);

        // 2. available tools (only tools WITH a snippet AND in the active set — R-06-012)
        out.push_str("\n\n");
        out.push_str(t.tools_header);
        out.push('\n');
        let mut any = false;
        for c in &inp.tool_contributions {
            if !is_selected(inp.selected_tools.as_ref(), &c.tool) {
                continue;
            }
            if let Some(s) = &c.snippet {
                out.push_str("- ");
                out.push_str(&c.tool);
                out.push_str(": ");
                out.push_str(s);
                out.push('\n');
                any = true;
            }
        }
        if !any {
            out.push_str(t.tools_empty);
            out.push('\n');
        }
        out.push_str(t.tools_extra);

        // 3. guidelines (dedup, insertion-order)
        out.push_str("\n\n");
        out.push_str(t.guidelines_header);
        out.push('\n');
        let mut seen: Vec<String> = Vec::new();
        // 3a. conditional file-exploration fallback
        let has = |n: &str| is_selected(inp.selected_tools.as_ref(), n);
        if has("bash") && !has("grep") && !has("find") && !has("ls") {
            push_guideline(out, &mut seen, t.bash_fallback_guideline);
        }
        // 3b. tool-specific guidelines (named per func-03 R-03-039)
        for c in &inp.tool_contributions {
            if !is_selected(inp.selected_tools.as_ref(), &c.tool) {
                continue;
            }
            for g in &c.guidelines {
                push_guideline(out, &mut seen, g);
            }
        }
        // 3c. caller-supplied extra guidelines
        for g in &inp.prompt_guidelines {
            push_guideline(out, &mut seen, g);
        }
        // 3d. baseline (always)
        for g in t.baseline_guidelines {
            push_guideline(out, &mut seen, g);
        }

        // 4. docs pointer (DI-4)
        emit_docs_section(out, t, &inp.docs);
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

/// Pi's four-tool fallback for an UNSET selection (`system-prompt.ts:81`).
pub const DEFAULT_SELECTED_TOOLS: &[&str] = &["read", "bash", "edit", "write"];

/// A tool is selected if the set is unset (Pi's `||` default) or explicitly names it.
///
/// An explicitly EMPTY set selects nothing — including `read`, which is what gates the skills
/// section (`system-prompt.ts:101,155`).
fn is_selected(selected: Option<&Vec<Arc<str>>>, name: &str) -> bool {
    match selected {
        None => DEFAULT_SELECTED_TOOLS.contains(&name),
        Some(v) => v.iter().any(|t| &**t == name),
    }
}

/// Push a deduped, trimmed guideline bullet (insertion order preserved).
fn push_guideline(out: &mut String, seen: &mut Vec<String>, g: &str) {
    let g = g.trim();
    if g.is_empty() || seen.iter().any(|s| s == g) {
        return;
    }
    seen.push(g.to_owned());
    out.push_str("- ");
    out.push_str(g);
    out.push('\n');
}

/// Pi `system-prompt.ts:131-138`, ported line-for-line. The three trailing bullets are
/// BEHAVIOURAL — they are what makes the pointers usable — and were missing entirely:
/// * `:135` resolve `docs/…` under Additional docs and `examples/…` under Examples, not the cwd;
/// * `:137` read the docs and examples, and follow `.md` cross-references before implementing;
/// * `:138` read `.md` files completely and follow links to related docs.
///
/// Pi's block sits inside the default-body template literal with **no guard**, so every default
/// prompt carries it; cyrup's paths are `Option`s and the block is skipped when all three are
/// absent. That guard is only reachable because the sole production caller still passes
/// `DocsPointers::default()` — see SESS-035; the path helpers themselves belong in `cyrup-config`
/// (Pi `config.ts:427-439`, three `resolve(join(getPackageDir(), …))` calls).
fn emit_docs_section(out: &mut String, t: &PromptTemplate, docs: &DocsPointers) {
    if docs.is_empty() {
        return;
    }
    out.push('\n');
    out.push_str(t.docs_header);
    if let Some(p) = &docs.readme {
        emit_docs_line(out, "Main documentation: ", p, "");
    }
    if let Some(p) = &docs.docs {
        emit_docs_line(out, "Additional docs: ", p, "");
    }
    if let Some(p) = &docs.examples {
        emit_docs_line(out, "Examples: ", p, " (extensions, custom tools, SDK)");
    }
    for line in t.docs_guidance {
        out.push_str("\n- ");
        out.push_str(line);
    }
}

fn emit_docs_line(out: &mut String, label: &str, p: &Path, suffix: &str) {
    out.push_str("\n- ");
    out.push_str(label);
    out.push_str(&p.to_string_lossy());
    out.push_str(suffix);
}

fn emit_context_files(out: &mut String, t: &PromptTemplate, files: &[ContextFile]) {
    if files.is_empty() {
        return;
    }
    out.push_str("\n\n");
    out.push_str(t.project_context_open);
    for cf in files {
        out.push_str("<project_instructions path=\"");
        out.push_str(&cf.path.to_string_lossy()); // lossy: never panic on non-UTF8
        out.push_str("\">\n");
        out.push_str(&cf.content);
        out.push_str("\n</project_instructions>\n\n");
    }
    out.push_str(t.project_context_close);
}

/// Pi's footer is exactly one line: `prompt += `\nCurrent working directory: ${promptCwd}``
/// (`system-prompt.ts:159`, and `:69` in the custom-prompt branch).
///
/// cyrup previously also emitted `"\n\nCurrent date: YYYY-MM-DD"` ahead of it. That was NOT a cyrup
/// invention — it is a faithful port of an older pi — but `git grep 'Current date' v0.83.0 --
/// packages/coding-agent/src` returns **nothing**, so the removal predates cyrup's own ported
/// baseline: carrying it was a stale port, not an upstream-drift decision. Removing it also removes
/// the extra leading `\n` (pi has one newline here, not two) and the only reason
/// [`SystemPromptBuilder::inputs_fingerprint`] hashed `today`, which forced a daily rebuild.
fn emit_footer(out: &mut String, inp: &PromptInputs) {
    use std::fmt::Write as _;
    let _ = write!(out, "\nCurrent working directory: {}", normalize_slashes(&inp.cwd));
}

fn normalize_slashes(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}
