//! arch-06 acceptance tests (A-06-1..8). Tolerant of clippy no-panic lints in test code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::RunCancel;
use cyrup_resources::SkillPointer;

use super::builder::{DocsPointers, PromptInputs, SystemPromptBuilder};
use super::cache::{ContextError, ContextStore};
use super::context_files::{ContextFile, ContextFileLoader, ContextScope, TrustQuery};
use super::hook::{
    apply_before_agent_start, BeforeAgentStartHook, BeforeAgentStartInput, BeforeAgentStartOutput,
};
use super::overrides::ResolvedOverride;
use super::tool_prompts::ToolPromptContribution;

fn date() -> time::Date {
    time::Date::from_calendar_date(2026, time::Month::June, 28).expect("valid date")
}

fn arc(s: &str) -> Arc<str> {
    Arc::from(s)
}

fn skill(name: &str, desc: &str, path: &str) -> SkillPointer {
    SkillPointer {
        name: name.to_string(),
        description: Some(desc.to_string()),
        path: PathBuf::from(path),
        disable_model_invocation: false,
    }
}

/// Same as [`skill`] but with `disable-model-invocation: true` frontmatter.
fn disabled_skill(name: &str, desc: &str, path: &str) -> SkillPointer {
    SkillPointer { disable_model_invocation: true, ..skill(name, desc, path) }
}

fn base_inputs() -> PromptInputs {
    PromptInputs { cwd: PathBuf::from("/work/proj"), today: date(), ..PromptInputs::default() }
}

struct Stub(bool);
impl TrustQuery for Stub {
    fn is_project_trusted(&self) -> bool {
        self.0
    }
}

// ── A-06-1: default composition with {read, bash} ───────────────────────────────────────────────
#[test]
fn a06_1_default_composition() {
    let inp = PromptInputs {
        selected_tools: Some(vec![arc("read"), arc("bash")]),
        tool_contributions: vec![
            ToolPromptContribution::snippet("read", "Read a file from disk"),
            ToolPromptContribution::snippet("bash", "Run a shell command"),
        ],
        docs: DocsPointers {
            readme: Some(PathBuf::from("/usr/share/cyrup/README.md")),
            ..DocsPointers::default()
        },
        skills: Arc::from(vec![skill("rustfmt", "format rust", "/skills/rustfmt/SKILL.md")]),
        ..base_inputs()
    };
    let out = SystemPromptBuilder::new().build(&inp);

    assert!(out.contains("operating inside cyrup"), "identity line\n{out}");
    assert!(out.contains("Available tools:"), "tools header");
    assert!(out.contains("- read: Read a file from disk"), "read snippet");
    assert!(out.contains("- bash: Run a shell command"), "bash snippet");
    assert!(out.contains("Guidelines:"));
    assert!(out.contains("- Be concise in your responses"), "baseline guideline");
    assert!(out.contains("cyrup documentation"), "docs pointer");
    assert!(out.contains("- Main documentation: /usr/share/cyrup/README.md"));
    // skills present because `read` is available
    assert!(out.contains("<available_skills>"), "skills section");
    assert!(out.contains("<name>rustfmt</name>"));
    // footer — SESS-019/DRIFT-035: pi's footer is the cwd line ALONE
    // (`system-prompt.ts:159`); `Current date:` is absent from `packages/coding-agent/src` at
    // v0.83.0, so asserting it pinned a stale port.
    assert!(!out.contains("Current date"), "pi emits no date line");
    assert!(out.ends_with("\nCurrent working directory: /work/proj"), "cwd footer\n{out}");
    // compact-ish: DI-1 sanity (well under a few KB for this tiny input)
    assert!(out.len() < 2048, "default prompt should stay compact, got {}", out.len());
}

// ── A-06-2: --system-prompt replaces body but keeps append + context + skills + footer ───────────
#[test]
fn a06_2_custom_prompt_keeps_tail() {
    let inp = PromptInputs {
        custom_prompt: Some(arc("REPLACED BODY ONLY")),
        selected_tools: Some(vec![arc("read")]),
        append_system_prompt: Some(arc("APPENDED EXTRA")),
        context_files: Arc::from(vec![ContextFile {
            path: PathBuf::from("/work/proj/AGENTS.md"),
            content: arc("project rules"),
            scope: ContextScope::Cwd,
        }]),
        skills: Arc::from(vec![skill("s1", "do s1", "/s1/SKILL.md")]),
        ..base_inputs()
    };
    let out = SystemPromptBuilder::new().build(&inp);

    assert!(out.starts_with("REPLACED BODY ONLY"), "custom body first\n{out}");
    assert!(!out.contains("operating inside cyrup"), "default identity removed");
    assert!(!out.contains("Available tools:"), "default tools removed");
    // tail still present
    assert!(out.contains("APPENDED EXTRA"), "append kept");
    assert!(out.contains("<project_context>"), "context kept");
    assert!(out.contains("project rules"));
    assert!(out.contains("<available_skills>"), "skills kept");
    assert!(out.ends_with("\nCurrent working directory: /work/proj"), "footer kept");
    // Pi's custom-prompt branch emits the same `<project_context>` wording and the same
    // `</project_context>\n` close as the default body (`system-prompt.ts:55-60` vs `:146-151`).
    assert!(
        out.contains("<project_context>\n\nProject-specific instructions and guidelines:\n\n"),
        "project_context wording matches system-prompt.ts:146-147\n{out}"
    );
    assert!(out.contains("</project_context>\n"), "close carries pi's trailing newline");
}

// ── A-06-3: APPEND_SYSTEM.md + repeatable --append-system-prompt all appended (no body removal) ──
#[test]
fn a06_3_append_accumulates() {
    let appended = ResolvedOverride::join_appends([
        "FROM_APPEND_SYSTEM_MD",
        "cli-append-one",
        "cli-append-two",
    ]);
    let inp = PromptInputs {
        selected_tools: Some(vec![arc("read")]),
        append_system_prompt: appended,
        ..base_inputs()
    };
    let out = SystemPromptBuilder::new().build(&inp);

    // default body still present
    assert!(out.contains("operating inside cyrup"), "default body kept");
    // all append sources present
    assert!(out.contains("FROM_APPEND_SYSTEM_MD"));
    assert!(out.contains("cli-append-one"));
    assert!(out.contains("cli-append-two"));
    // order preserved
    let i0 = out.find("FROM_APPEND_SYSTEM_MD").unwrap();
    let i1 = out.find("cli-append-one").unwrap();
    let i2 = out.find("cli-append-two").unwrap();
    assert!(i0 < i1 && i1 < i2, "append precedence order");
}

// ── A-06-4: context discovery order/first-found + -nc ────────────────────────────────────────────
#[test]
fn a06_4_context_discovery_order_and_nc() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let global = root.join("global-agent");
    let parent = root.join("parent");
    let cwd = parent.join("child");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(global.join("AGENTS.md"), "GLOBAL").unwrap();
    std::fs::write(parent.join("AGENTS.md"), "PARENT").unwrap();
    // cwd has BOTH — AGENTS.md must win over CLAUDE.md (first-found)
    std::fs::write(cwd.join("AGENTS.md"), "CWD_AGENTS").unwrap();
    std::fs::write(cwd.join("CLAUDE.md"), "CWD_CLAUDE").unwrap();

    let loader = ContextFileLoader::new(cwd.clone(), global.clone(), true, false);
    let (files, _diags) = loader.load();
    let contents: Vec<&str> = files.iter().map(|f| &*f.content).collect();
    assert_eq!(contents, vec!["GLOBAL", "PARENT", "CWD_AGENTS"], "global→parent→cwd, AGENTS wins");
    assert_eq!(files[0].scope, ContextScope::Global);
    assert_eq!(files[2].scope, ContextScope::Cwd);

    // -nc disables everything
    let disabled = ContextFileLoader::new(cwd, global, true, true);
    let (files, diags) = disabled.load();
    assert!(files.is_empty() && diags.is_empty(), "-nc loads nothing");
}

// ── A-06-4b: `AGENTS.override.md` is the FIRST candidate and WINS its directory ──────────────────
//
// Pi's `loadContextFileFromDir` returns on the first existing candidate, so the array position is
// the whole mechanism: `["AGENTS.override.md", "AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"]`
// (`v0.84.1 coding-agent/src/core/resource-loader.ts:71-88`). Added upstream in `8ecf8a988` (#7681,
// 2026-08-05), after the ported v0.83.0 baseline, whose array was the 4-entry
// `["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"]`
// (`v0.83.0 coding-agent/src/core/resource-loader.ts:71`) — version lag, not a port bug.
#[test]
fn a06_4b_agents_override_wins_over_agents_md() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let global = root.join("global-agent");
    let parent = root.join("parent");
    let cwd = parent.join("child");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();

    // Global dir: override present alongside AGENTS.md -> override wins here too (Pi applies the
    // same `loadContextFileFromDir` to every scope, `resource-loader.ts:118+`).
    std::fs::write(global.join("AGENTS.override.md"), "GLOBAL_OVERRIDE").unwrap();
    std::fs::write(global.join("AGENTS.md"), "GLOBAL_AGENTS").unwrap();
    // Parent dir: NO override -> plain AGENTS.md still used (mirror: prepending a candidate must
    // not disturb the pre-existing four or their order).
    std::fs::write(parent.join("AGENTS.md"), "PARENT_AGENTS").unwrap();
    // cwd: override beats AGENTS.md *and* CLAUDE.md.
    std::fs::write(cwd.join("AGENTS.override.md"), "CWD_OVERRIDE").unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "CWD_AGENTS").unwrap();
    std::fs::write(cwd.join("CLAUDE.md"), "CWD_CLAUDE").unwrap();

    let loader = ContextFileLoader::new(cwd, global, true, false);
    let (files, _diags) = loader.load();
    let contents: Vec<&str> = files.iter().map(|f| &*f.content).collect();
    assert_eq!(
        contents,
        vec!["GLOBAL_OVERRIDE", "PARENT_AGENTS", "CWD_OVERRIDE"],
        "AGENTS.override.md wins its dir; a dir without one still resolves AGENTS.md"
    );
    // First-found is exclusive: the shadowed AGENTS.md/CLAUDE.md are NOT also loaded.
    assert_eq!(files.len(), 3, "one file per directory, never two");
    assert!(
        files.iter().all(|f| f.content.as_ref() != "CWD_AGENTS"),
        "AGENTS.md must be shadowed by AGENTS.override.md, not appended alongside it"
    );
    assert!(files[0].path.ends_with("AGENTS.override.md"), "global resolved to the override");
    assert!(files[2].path.ends_with("AGENTS.override.md"), "cwd resolved to the override");
}

// ── A-06-4c: MIRROR — `AGENTS.override.md` does not outrank a NEARER scope ───────────────────────
//
// Position within `CANDIDATES` orders candidates inside ONE directory; it does not reorder the
// global→ancestors→cwd concatenation (`resource-loader.ts:118+`). An override in an ancestor must
// therefore still be listed BEFORE (i.e. outranked by) a plain `CLAUDE.md` in cwd.
#[test]
fn a06_4c_override_does_not_reorder_scopes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let global = root.join("global-agent");
    let parent = root.join("parent");
    let cwd = parent.join("child");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(parent.join("AGENTS.override.md"), "PARENT_OVERRIDE").unwrap();
    std::fs::write(cwd.join("CLAUDE.MD"), "CWD_CLAUDE_UPPER").unwrap();

    let loader = ContextFileLoader::new(cwd, global, true, false);
    let (files, _diags) = loader.load();
    let contents: Vec<&str> = files.iter().map(|f| &*f.content).collect();
    assert_eq!(
        contents,
        vec!["PARENT_OVERRIDE", "CWD_CLAUDE_UPPER"],
        "scope order (ancestor→cwd) is unaffected by candidate order"
    );
}

// ── A-06-5: untrusted project skips project AGENTS.md but loads global ───────────────────────────
#[test]
fn a06_5_untrusted_skips_project_keeps_global() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let global = root.join("global-agent");
    let cwd = root.join("proj");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(global.join("AGENTS.md"), "GLOBAL").unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "PROJECT").unwrap();

    let trust = Stub(false);
    let loader = ContextFileLoader::from_trust(cwd, global, &trust, false);
    let (files, _diags) = loader.load();
    let contents: Vec<&str> = files.iter().map(|f| &*f.content).collect();
    assert_eq!(contents, vec!["GLOBAL"], "only global loaded for untrusted project");
}

// ── A-06-6: read-tool gates skills section; empty skills (--no-skills) removes it regardless ─────
#[test]
fn a06_6_read_gates_skills() {
    let with_skills = vec![skill("s1", "use s1", "/s1/SKILL.md")];

    // read available -> skills present
    let inp = PromptInputs {
        selected_tools: Some(vec![arc("read"), arc("bash")]),
        skills: Arc::from(with_skills.clone()),
        ..base_inputs()
    };
    assert!(SystemPromptBuilder::new().build(&inp).contains("<available_skills>"));

    // read NOT available -> no skills section even with skills loaded
    let inp_no_read = PromptInputs {
        selected_tools: Some(vec![arc("bash")]),
        skills: Arc::from(with_skills),
        ..base_inputs()
    };
    assert!(!SystemPromptBuilder::new().build(&inp_no_read).contains("<available_skills>"));

    // read available but --no-skills (empty set) -> no section
    let inp_no_skills = PromptInputs {
        selected_tools: Some(vec![arc("read")]),
        skills: Arc::from(Vec::new()),
        ..base_inputs()
    };
    assert!(!SystemPromptBuilder::new().build(&inp_no_skills).contains("<available_skills>"));
}

// ── SESS-003: `disable-model-invocation` skills are excluded from `<available_skills>` ───────────
// Pi `formatSkillsForPrompt` (skills.ts:334-339): `skills.filter((s) => !s.disableModelInvocation)`
// and an empty visible set returns "" (no section at all).
#[test]
fn sess003_disabled_skills_are_not_advertised_to_the_model() {
    // Mixed set: only the enabled skill reaches the prompt.
    let inp = PromptInputs {
        selected_tools: Some(vec![arc("read")]),
        skills: Arc::from(vec![
            skill("visible-skill", "the model may use this", "/s/visible/SKILL.md"),
            disabled_skill("hidden-skill", "explicit invocation only", "/s/hidden/SKILL.md"),
        ]),
        ..base_inputs()
    };
    let out = SystemPromptBuilder::new().build(&inp);
    assert!(out.contains("<available_skills>"), "section emitted for the enabled skill");
    assert!(out.contains("<name>visible-skill</name>"), "enabled skill advertised");
    assert!(
        !out.contains("hidden-skill"),
        "disable-model-invocation skill must not appear in the prompt; got:\n{out}"
    );
    assert!(
        !out.contains("/s/hidden/SKILL.md"),
        "disabled skill's location must not leak either; got:\n{out}"
    );

    // Only-disabled set: no section at all (not an empty one).
    let inp_all_disabled = PromptInputs {
        selected_tools: Some(vec![arc("read")]),
        skills: Arc::from(vec![
            disabled_skill("hidden-a", "explicit only", "/s/a/SKILL.md"),
            disabled_skill("hidden-b", "explicit only", "/s/b/SKILL.md"),
        ]),
        ..base_inputs()
    };
    let out_all = SystemPromptBuilder::new().build(&inp_all_disabled);
    assert!(
        !out_all.contains("<available_skills>"),
        "an all-disabled set emits no skills section; got:\n{out_all}"
    );
}

// ── SESS-016: an explicitly EMPTY selected-tools list selects NOTHING ────────────────────────────
//
// Pi: `const tools = selectedTools || ["read","bash","edit","write"]` (`system-prompt.ts:81`). An
// empty array is TRUTHY in JS, so `tools` stays `[]`, `hasRead` (`:101`) is false and the skills
// gate at `:155` skips; the custom-prompt branch is the same predicate at `:64`
// (`!selectedTools || selectedTools.includes("read")`). Reachable in production:
// `cyrup-session-svc/src/tools.rs:61` sets the field from the live active-tool set, so disabling
// every tool lands here.
#[test]
fn sess016_explicitly_empty_tool_set_emits_no_skills_and_no_tool_guidelines() {
    let skills = vec![skill("s1", "use s1", "/s1/SKILL.md")];
    let read_snippet = ToolPromptContribution::snippet("read", "Read a file from disk");
    let bash = ToolPromptContribution::snippet("bash", "Run a shell command")
        .with_guideline("Quote your bash arguments");

    // Some(vec![]) == "the caller restricted the agent to zero tools".
    let empty = PromptInputs {
        selected_tools: Some(Vec::new()),
        tool_contributions: vec![read_snippet.clone(), bash.clone()],
        skills: Arc::from(skills.clone()),
        ..base_inputs()
    };
    let out = SystemPromptBuilder::new().build(&empty);
    assert!(!out.contains("<available_skills>"), "no skills without `read`; got:\n{out}");
    assert!(!out.contains("- read: Read a file from disk"), "no snippet for an unselected tool");
    assert!(!out.contains("Quote your bash arguments"), "no guideline for an unselected tool");
    assert!(
        !out.contains("Use bash for file operations"),
        "the bash-fallback guideline needs `bash` selected, and it is not"
    );
    assert!(out.contains("(none)"), "empty tool list renders pi's `(none)` placeholder");

    // None == UNSET == pi's four-tool default, which DOES include `read`.
    let unset = PromptInputs {
        selected_tools: None,
        tool_contributions: vec![read_snippet, bash],
        skills: Arc::from(skills),
        ..base_inputs()
    };
    let out_unset = SystemPromptBuilder::new().build(&unset);
    assert!(out_unset.contains("<available_skills>"), "unset falls back to pi's default set");
    assert!(out_unset.contains("- read: Read a file from disk"));
    assert!(out_unset.contains("Quote your bash arguments"), "`bash` is in pi's default set");
    // ...but `grep` is not, so an unset set must not select an arbitrary name.
    let grep_only = PromptInputs {
        selected_tools: None,
        tool_contributions: vec![ToolPromptContribution::snippet("grep", "search")],
        ..base_inputs()
    };
    assert!(
        !SystemPromptBuilder::new().build(&grep_only).contains("- grep: search"),
        "the default set is exactly [read, bash, edit, write] (system-prompt.ts:81)"
    );
}

// ── SESS-024: the skills preamble carries pi's relative-path resolution instruction ──────────────
// `skills.ts:342-347` builds five preamble strings; `:345` is the behavioural one.
#[test]
fn sess024_skills_preamble_carries_pi_relative_path_rule() {
    let inp = PromptInputs {
        selected_tools: Some(vec![arc("read")]),
        skills: Arc::from(vec![skill("s1", "use s1", "/s/s1/SKILL.md")]),
        ..base_inputs()
    };
    let out = SystemPromptBuilder::new().build(&inp);
    assert!(
        out.contains(
            "When a skill file references a relative path, resolve it against the skill directory \
             (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands."
        ),
        "skills.ts:345 verbatim; got:\n{out}"
    );
    assert!(
        out.contains("The following skills provide specialized instructions for specific tasks."),
        "skills.ts:343"
    );
    assert!(
        out.contains("Use the read tool to load a skill's file when the task matches its description."),
        "skills.ts:344"
    );
    // `:346` is the empty string, i.e. a blank line immediately before the block.
    assert!(out.contains("\n\n<available_skills>\n"), "blank line before the block (skills.ts:346)");
}

// ── SESS-035: the docs section carries pi's three behavioural bullets ────────────────────────────
// `system-prompt.ts:135`, `:137`, `:138`. (The production WIRING — populating `DocsPointers` from
// package-relative paths, pi `config.ts:427-439` — lands in `cyrup-config`/`cyrup-session-svc` and
// is NOT covered here; this pins the emitter half only.)
#[test]
fn sess035_docs_section_emits_pi_resolution_and_cross_reference_rules() {
    let inp = PromptInputs {
        selected_tools: Some(vec![arc("read")]),
        docs: DocsPointers {
            readme: Some(PathBuf::from("/pkg/README.md")),
            docs: Some(PathBuf::from("/pkg/docs")),
            examples: Some(PathBuf::from("/pkg/examples")),
        },
        ..base_inputs()
    };
    let out = SystemPromptBuilder::new().build(&inp);
    assert!(out.contains("- Main documentation: /pkg/README.md"));
    assert!(out.contains("- Additional docs: /pkg/docs"));
    assert!(
        out.contains("- Examples: /pkg/examples (extensions, custom tools, SDK)"),
        "pi's parenthetical rides on the Examples line (system-prompt.ts:134)"
    );
    assert!(
        out.contains(
            "resolve docs/... under Additional docs and examples/... under Examples, not the \
             current working directory"
        ),
        "system-prompt.ts:135; got:\n{out}"
    );
    assert!(
        out.contains("follow .md cross-references before implementing"),
        "system-prompt.ts:137"
    );
    assert!(
        out.contains("read cyrup .md files completely and follow links to related docs"),
        "system-prompt.ts:138"
    );

    // All-absent stays silent (the guard is only reachable while SESS-035's wiring is missing).
    let none = PromptInputs { selected_tools: Some(vec![arc("read")]), ..base_inputs() };
    assert!(!SystemPromptBuilder::new().build(&none).contains("cyrup documentation"));
}

// ── SESS-033: the fingerprint covers `disable_model_invocation` ──────────────────────────────────
#[test]
fn sess033_fingerprint_tracks_disable_model_invocation() {
    let builder = SystemPromptBuilder::new();
    let visible = PromptInputs {
        selected_tools: Some(vec![arc("read")]),
        skills: Arc::from(vec![skill("s1", "d", "/s/s1/SKILL.md")]),
        ..base_inputs()
    };
    let hidden = PromptInputs {
        skills: Arc::from(vec![disabled_skill("s1", "d", "/s/s1/SKILL.md")]),
        ..visible.clone()
    };
    // The two prompts genuinely differ, so their fingerprints must too.
    assert_ne!(builder.build(&visible), builder.build(&hidden));
    assert_ne!(
        builder.inputs_fingerprint(&visible),
        builder.inputs_fingerprint(&hidden),
        "flipping disable-model-invocation must invalidate a prompt cache"
    );

    // UNSET vs explicitly-EMPTY tools are different prompts and must hash differently (SESS-016).
    let unset = PromptInputs { selected_tools: None, ..visible.clone() };
    let empty = PromptInputs { selected_tools: Some(Vec::new()), ..visible };
    assert_ne!(builder.inputs_fingerprint(&unset), builder.inputs_fingerprint(&empty));

    // `today` no longer affects any byte, so it must NOT force a daily rebuild (SESS-019/033).
    let d1 = PromptInputs {
        today: time::Date::from_calendar_date(2026, time::Month::June, 28).expect("date"),
        ..base_inputs()
    };
    let d2 = PromptInputs {
        today: time::Date::from_calendar_date(2027, time::Month::January, 1).expect("date"),
        ..base_inputs()
    };
    assert_eq!(builder.build(&d1), builder.build(&d2), "no date reaches the prompt");
    assert_eq!(builder.inputs_fingerprint(&d1), builder.inputs_fingerprint(&d2));
}

// ── A-06-7: dynamic tool snippet/guideline appears then disappears; fingerprint changes ──────────
#[test]
fn a06_7_dynamic_tool_snippet() {
    let builder = SystemPromptBuilder::new();
    let dynamic = ToolPromptContribution::snippet("mytool", "does a dynamic thing")
        .with_guideline("Prefer mytool for dynamic things");

    let with_tool = PromptInputs {
        selected_tools: Some(vec![arc("read"), arc("mytool")]),
        tool_contributions: vec![
            ToolPromptContribution::snippet("read", "Read a file"),
            dynamic,
        ],
        ..base_inputs()
    };
    let out_with = builder.build(&with_tool);
    assert!(out_with.contains("- mytool: does a dynamic thing"), "dynamic snippet present");
    assert!(out_with.contains("- Prefer mytool for dynamic things"), "dynamic guideline present");

    // tool disabled: removed from active set AND contributions
    let without_tool = PromptInputs {
        selected_tools: Some(vec![arc("read")]),
        tool_contributions: vec![ToolPromptContribution::snippet("read", "Read a file")],
        ..base_inputs()
    };
    let out_without = builder.build(&without_tool);
    assert!(!out_without.contains("mytool"), "dynamic tool gone");
    assert!(!out_without.contains("dynamic things"), "dynamic guideline gone");

    assert_ne!(
        builder.inputs_fingerprint(&with_tool),
        builder.inputs_fingerprint(&without_tool),
        "fingerprint detects active-set change -> triggers rebuild"
    );
}

// ── A-06-8: before_agent_start hook replaces the prompt; trapping hook degrades to pre-hook ──────
struct ReplaceHook;
impl BeforeAgentStartHook for ReplaceHook {
    fn before_agent_start(&self, input: &BeforeAgentStartInput) -> BeforeAgentStartOutput {
        // sanity: hook receives build options (R-06-014)
        assert_eq!(input.cwd, PathBuf::from("/work/proj"));
        BeforeAgentStartOutput::replace("HOOK REPLACED PROMPT")
    }
}
struct AppendHook;
impl BeforeAgentStartHook for AppendHook {
    fn before_agent_start(&self, input: &BeforeAgentStartInput) -> BeforeAgentStartOutput {
        BeforeAgentStartOutput::replace(format!("{}\n[rules]", input.system_prompt))
    }
}
struct KeepHook;
impl BeforeAgentStartHook for KeepHook {
    fn before_agent_start(&self, _: &BeforeAgentStartInput) -> BeforeAgentStartOutput {
        BeforeAgentStartOutput::keep()
    }
}

#[test]
fn a06_8_before_agent_start_replaces_prompt() {
    let inp = PromptInputs { selected_tools: Some(vec![arc("read")]), ..base_inputs() };
    let built = SystemPromptBuilder::new().build(&inp);

    // replacement wins
    let final_prompt = apply_before_agent_start(built.clone(), &inp, &[&ReplaceHook]);
    assert_eq!(final_prompt, "HOOK REPLACED PROMPT");

    // keep-hook leaves it untouched
    let unchanged = apply_before_agent_start(built.clone(), &inp, &[&KeepHook]);
    assert_eq!(unchanged, built);

    // R-06-015 append-style composes on top, in subscription order
    let composed = apply_before_agent_start(built.clone(), &inp, &[&KeepHook, &AppendHook]);
    assert!(composed.starts_with(&built) && composed.ends_with("[rules]"));
}

// ── ContextStore: read-once-per-session cache + spawn_blocking reload (R-06-016) ─────────────────
#[tokio::test]
async fn context_store_reload_caches_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("proj");
    let global = tmp.path().join("global-agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "PROJECT RULES").unwrap();

    let store = ContextStore::new();
    assert!(store.snapshot().context_files.is_empty(), "empty before reload");

    let loader = ContextFileLoader::new(cwd, global, true, false);
    let skills: Arc<[SkillPointer]> = Arc::from(vec![skill("s", "d", "/s/SKILL.md")]);
    let cancel = RunCancel::new();
    store
        .reload(&cancel, loader, skills, ResolvedOverride::default())
        .await
        .expect("reload ok");

    let snap = store.snapshot();
    assert_eq!(snap.context_files.len(), 1);
    assert_eq!(&*snap.context_files[0].content, "PROJECT RULES");
    assert_eq!(snap.skills.len(), 1);
}

#[tokio::test]
async fn context_store_reload_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let loader = ContextFileLoader::new(tmp.path().to_path_buf(), tmp.path().to_path_buf(), true, false);
    let cancel = RunCancel::new();
    cancel.cancel();
    let err = ContextStore::new()
        .reload(&cancel, loader, Arc::from(Vec::new()), ResolvedOverride::default())
        .await
        .expect_err("cancelled");
    assert!(matches!(err, ContextError::Cancelled));
}
