//! Prompt-template workflows — a 1:1 port of `pi-subagents/src/slash/prompt-workflows.ts` (330
//! lines @v0.34.0), the subsystem that turns a `prompts/*.md` recipe into a runnable subagent
//! delegation and exposes it to the user as `/prompt-workflow` and `/chain-prompts`.
//!
//! # Why this module exists (the gap it closes)
//!
//! [`super::resources::bundled_prompt_files`] discovers the SEVEN `.md` recipes this crate ships
//! under `resources/prompts/` — and until this module landed its ONLY caller was
//! `resources.rs`'s own `#[cfg(test)]` block. The recipes were vendored, discovered and unit-tested,
//! and no user could invoke one: nothing registered a command that reads them. Upstream reaches
//! them through two slash commands registered from `registerSlashCommands`
//! (`slash/slash-commands.ts:1099-1102` @v0.34.0 calls `registerPromptWorkflowCommands`, which
//! registers `prompt-workflow` at `prompt-workflows.ts:269` and `chain-prompts` at `:303`), and
//! `registerSlashCommands(pi, state)` is itself called from the extension entry point
//! (`extension/index.ts:529`). So the user action is literally typing `/prompt-workflow list` or
//! `/prompt-workflow parallel-review <task>`.
//!
//! Classification: **port-bug**. Both commands and this whole file exist at the ported baseline
//! v0.34.0; `git -C pi-subagents show v0.34.0:src/slash/prompt-workflows.ts` is byte-identical in
//! every function this module ports to the v0.38.0 copy.
//!
//! # Discovery — three tiers, package first (`promptDirs`, `:41-47`)
//!
//! 1. the extension package's own `prompts/` directory (`packagePromptsDir()`, `:37-39`) — cyrup's
//!    analog is [`super::resources::bundled_prompt_files`], which resolves the SAME bundled root
//!    through `cyrup_resources::resolve_manifest`;
//! 2. `<agentDir>/prompts` (`getAgentDir()`, `shared/utils.ts:72-77` — cyrup: `CYRUP_AGENT_DIR`/
//!    `PI_CODING_AGENT_DIR`, else `<home>/.cyrup/agent`);
//! 3. `<cwd>/.cyrup/prompts` (`getProjectConfigDir(cwd)`, `shared/utils.ts:68-70`, whose argument
//!    at this call site is `ctx.cwd` verbatim — NOT a discovered project root).
//!
//! Later tiers overwrite earlier ones by name (`workflows.set(workflow.name, …)`, `:123`), so a
//! project recipe shadows a user one shadows the bundled one. The final list is sorted by name
//! (`:125`).
//!
//! [CYRUP-DELTA] `readPromptFiles` (`:49-63`) does a FLAT `readdirSync` of each directory, while
//! `bundled_prompt_files()` expands the manifest entry recursively. For the bundled root — seven
//! flat `.md` files — the two agree exactly; the user/project tiers below use the flat walk
//! upstream specifies. Routing tier 1 through the manifest is deliberate: it keeps ONE definition
//! of "which files does this crate ship" instead of a second `resources/prompts` path literal.

use std::path::{Path, PathBuf};

use crate::discovery::frontmatter::parse_frontmatter_block;
use crate::fork_context::ContextMode;

/// Command names a prompt file may not claim (`RESERVED_COMMAND_NAMES`, `prompt-workflows.ts:26-35`
/// @v0.34.0). A `prompts/run.md` would otherwise shadow `/run`; upstream drops it at load
/// (`:98`) rather than registering it. Ported as upstream's exact eight, not as cyrup's own
/// thirteen-command table — this is the upstream-declared reservation list, and widening it would
/// silently reject recipe names upstream accepts.
const RESERVED_COMMAND_NAMES: &[&str] = &[
    "chain-prompts",
    "prompt-workflow",
    "run",
    "chain",
    "parallel",
    "run-chain",
    "subagents-doctor",
    "subagents-models",
];

/// The default persona a recipe delegates to when its frontmatter names none (`parseAgent`,
/// `prompt-workflows.ts:88-92`: an absent `subagent:` — or the literal `true` — means `delegate`).
pub const DEFAULT_WORKFLOW_AGENT: &str = "delegate";

/// One discovered prompt recipe (`interface PromptWorkflow`, `prompt-workflows.ts:10-22`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptWorkflow {
    /// The file's basename without `.md` — the name the user types after `/prompt-workflow`.
    pub name: String,
    /// `description:` frontmatter, else the body's first non-empty line (`firstNonEmptyLine`,
    /// `:65-67`, which itself falls back to the literal `"Prompt workflow"`).
    pub description: String,
    /// The markdown body, before `$ARGUMENTS`/`$N` substitution.
    pub body: String,
    /// Absolute path this recipe was loaded from (rendered by [`format_workflow_list`]).
    pub file_path: PathBuf,
    /// Resolved persona name (`parseAgent`, `:88-92`).
    pub agent: String,
    /// `inheritContext:`/`fork:` → [`ContextMode::Fork`]; `fresh:` → [`ContextMode::Fresh`]
    /// (`:109-110`). `None` defers to the persona's own default context.
    pub context: Option<ContextMode>,
    /// `model:` frontmatter override.
    pub model: Option<String>,
    /// `skill:` frontmatter (`parseSkill`, `:81-86`), already normalized to cyrup's
    /// [`crate::extension::SingleRunOverrides::skills`] tri-state: `None` = inherit the persona's
    /// list, `Some(vec![])` = upstream's `skill: false`, `Some(names)` = replace it.
    pub skills: Option<Vec<String>>,
    /// `cwd:` frontmatter — a child working directory, resolved against the session cwd.
    pub cwd: Option<String>,
    /// `worktree: true` frontmatter.
    pub worktree: bool,
    /// `chain:` frontmatter — an ` -> `-separated list of OTHER recipe names this one expands to
    /// (`:102`, consumed at `:286-295`).
    pub chain: Option<String>,
}

// =================================================================================================
// Discovery (promptDirs / readPromptFiles / loadPromptWorkflow / discoverPromptWorkflows)
// =================================================================================================

/// The user home dir, mirroring this crate's existing `extension.rs::dirs_home` /
/// `exec::mcp_direct_tools::home_dir` convention (`CYRUP_HOME` → `HOME` → tempdir).
fn home_dir() -> PathBuf {
    std::env::var_os("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// pi `getAgentDir()` (`shared/utils.ts:72-77`): `CYRUP_AGENT_DIR`/`PI_CODING_AGENT_DIR` with `~`
/// expansion, else `<home>/.cyrup/agent`. Kept identical to
/// `exec::mcp_direct_tools::resolve_agent_dir` — the same upstream function, and the two must not
/// disagree about where the agent dir is.
fn agent_dir() -> PathBuf {
    let home = home_dir();
    let configured = std::env::var("CYRUP_AGENT_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("PI_CODING_AGENT_DIR").ok().filter(|v| !v.is_empty()));
    match configured {
        Some(v) if v == "~" => home,
        Some(v) if v.starts_with("~/") => home.join(v.get(2..).unwrap_or("")),
        Some(v) => PathBuf::from(v),
        None => home.join(".cyrup").join("agent"),
    }
}

/// The two NON-package prompt directories, in upstream's order (`promptDirs`, `:41-47`, minus the
/// package tier which [`prompt_files`] takes from the bundled manifest instead):
/// `<agentDir>/prompts`, then `<cwd>/.cyrup/prompts`.
fn user_and_project_prompt_dirs(cwd: &Path) -> Vec<PathBuf> {
    vec![agent_dir().join("prompts"), cwd.join(".cyrup").join("prompts")]
}

/// Every candidate prompt file, package tier first (`readPromptFiles`, `:49-63`). Package files
/// come from [`super::resources::bundled_prompt_files`]; the user/project tiers are a FLAT
/// directory read of `*.md`, exactly as upstream's `readdirSync` + `entry.isFile()` walk. A missing
/// directory contributes nothing (upstream's `catch { continue; }`).
fn prompt_files(cwd: &Path) -> Vec<PathBuf> {
    let mut files = super::resources::bundled_prompt_files();
    for dir in user_and_project_prompt_dirs(cwd) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
            .collect();
        // `readdirSync` order is filesystem-dependent; sorting keeps a directory's own
        // last-one-wins outcome deterministic without changing the TIER order that matters.
        found.sort();
        files.extend(found);
    }
    files
}

/// pi `firstNonEmptyLine` (`:65-67`): the first line with non-whitespace content, trimmed; the
/// literal `"Prompt workflow"` when there is none.
fn first_non_empty_line(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Prompt workflow")
        .to_string()
}

/// pi `booleanField` (`:74-79`): `true`/`yes`/`1` → `Some(true)`, `false`/`no`/`0` → `Some(false)`,
/// anything else (including an absent key) → `None`. Case-insensitive.
fn boolean_field(value: Option<&str>) -> Option<bool> {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("true" | "yes" | "1") => Some(true),
        Some("false" | "no" | "0") => Some(false),
        _ => None,
    }
}

/// pi `parseSkill` (`:81-86`) folded onto cyrup's normalized tri-state: an absent/empty value is
/// `None` (inherit); the literal `"false"` is `Some(vec![])` (upstream's `false`); otherwise the
/// comma-separated, trimmed, non-empty names.
fn parse_skill(value: Option<&str>) -> Option<Vec<String>> {
    let value = value.map(str::trim).filter(|v| !v.is_empty())?;
    if value == "false" {
        return Some(Vec::new());
    }
    let parts: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    // Upstream returns `parts[0]` for a single entry and the array otherwise; both lower onto the
    // same `Vec<String>` here. An all-separator value (`",,"`) yields `parts[0] === undefined`
    // upstream, i.e. `skill: undefined` — `None`, not the empty list.
    if parts.is_empty() { None } else { Some(parts) }
}

/// pi `loadPromptWorkflow` (`:94-117`): read + parse one recipe. `None` for an unreadable file, an
/// empty basename, or a [`RESERVED_COMMAND_NAMES`] collision (`:98`).
fn load_prompt_workflow(file_path: &Path) -> Option<PromptWorkflow> {
    let content = std::fs::read_to_string(file_path).ok()?;
    let parsed = parse_frontmatter_block(&content);
    let name = file_path.file_stem().and_then(|s| s.to_str())?.to_string();
    if name.is_empty() || RESERVED_COMMAND_NAMES.contains(&name.as_str()) {
        return None;
    }
    let field = |key: &str| parsed.get(key).map(str::trim).filter(|v| !v.is_empty());

    // `:109-110` — `inheritContext`/`fork` select FORK, `fresh` selects FRESH, and upstream spreads
    // the fresh clause SECOND, so an explicit `fresh: true` wins over `fork: true` on one file.
    let mut context = None;
    if boolean_field(field("inheritContext")) == Some(true) || boolean_field(field("fork")) == Some(true) {
        context = Some(ContextMode::Fork);
    }
    if boolean_field(field("fresh")) == Some(true) {
        context = Some(ContextMode::Fresh);
    }

    // `parseAgent` (`:88-92`): absent, or the literal string `true`, means `delegate`.
    let agent = match field("subagent") {
        None | Some("true") => DEFAULT_WORKFLOW_AGENT.to_string(),
        Some(other) => other.to_string(),
    };

    Some(PromptWorkflow {
        description: field("description")
            .map(str::to_string)
            .unwrap_or_else(|| first_non_empty_line(&parsed.body)),
        body: parsed.body.clone(),
        file_path: file_path.to_path_buf(),
        agent,
        context,
        model: field("model").map(str::to_string),
        skills: parse_skill(field("skill")),
        cwd: field("cwd").map(str::to_string),
        worktree: boolean_field(field("worktree")) == Some(true),
        chain: field("chain").map(str::to_string),
        name,
    })
}

/// pi `discoverPromptWorkflows` (`:119-126`): load every candidate file, LAST tier wins per name,
/// then sort by name.
#[must_use]
pub fn discover_prompt_workflows(cwd: &Path) -> Vec<PromptWorkflow> {
    let mut by_name: Vec<PromptWorkflow> = Vec::new();
    for file in prompt_files(cwd) {
        let Some(workflow) = load_prompt_workflow(&file) else {
            continue;
        };
        match by_name.iter_mut().find(|w| w.name == workflow.name) {
            Some(slot) => *slot = workflow,
            None => by_name.push(workflow),
        }
    }
    by_name.sort_by(|a, b| a.name.cmp(&b.name));
    by_name
}

/// pi `findWorkflow` (`:251-253`): exact-name lookup.
#[must_use]
pub fn find_workflow<'a>(workflows: &'a [PromptWorkflow], name: &str) -> Option<&'a PromptWorkflow> {
    workflows.iter().find(|w| w.name == name)
}

/// pi `formatWorkflowList` (`:255-261`), verbatim wording including the empty-list sentence.
#[must_use]
pub fn format_workflow_list(workflows: &[PromptWorkflow]) -> String {
    if workflows.is_empty() {
        return "No prompt workflows found in package, user, or project prompts.".to_string();
    }
    let mut out = vec!["Prompt workflows:".to_string()];
    for w in workflows {
        out.push(format!("- {}: {} ({})", w.name, w.description, w.file_path.display()));
    }
    out.join("\n")
}

// =================================================================================================
// Argument grammar (shellWords / substituteArgs / parseRuntimeOptions / the two splitters)
// =================================================================================================

/// pi `shellWords` (`:128-163`): whitespace-split with `'`/`"` quoting and `\` escaping. A quote
/// character is consumed, never emitted; an unterminated quote runs to end of input.
#[must_use]
pub fn shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// One positional argument by 1-based index, or `""` when absent (`args[Number(index) - 1] ?? ""`,
/// `:171`).
fn positional(args: &[String], index: usize) -> &str {
    index
        .checked_sub(1)
        .and_then(|i| args.get(i))
        .map(String::as_str)
        .unwrap_or_default()
}

/// pi `substituteArgs` (`:165-172`), as four sequential passes in upstream's order: `$ARGUMENTS`,
/// `$@`, `${N:-fallback}`, then `$N`.
///
/// The `${N:-fallback}` pass uses JS `||`, so an argument that was supplied but EMPTY still takes
/// the fallback; the bare `$N` pass uses `??`, so an absent argument becomes `""`. The two are not
/// the same rule and the difference is observable — `${1:-all}` with `""` yields `all`, `$1` yields
/// `""`.
#[must_use]
pub fn substitute_args(template: &str, args: &[String]) -> String {
    let all = args.join(" ");
    let mut out = template.replace("$ARGUMENTS", &all).replace("$@", &all);
    out = replace_defaulted_positionals(&out, args);
    replace_bare_positionals(&out, args)
}

/// The `\$\{(\d+):-([^}]*)\}` pass of [`substitute_args`].
fn replace_defaulted_positionals(input: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(at) = rest.find("${") {
        let (head, tail) = rest.split_at(at);
        out.push_str(head);
        // `tail` starts with "${"; find the closing brace of THIS `${…}` — the regex's `[^}]*`
        // cannot span a `}`, so the first one closes it.
        let Some(close) = tail.find('}') else {
            out.push_str(tail);
            return out;
        };
        let inner = tail.get(2..close).unwrap_or_default();
        match inner.split_once(":-") {
            Some((digits, fallback))
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) =>
            {
                let index = digits.parse::<usize>().unwrap_or(0);
                let value = positional(args, index);
                // JS `||`: an empty supplied argument still takes the fallback.
                out.push_str(if value.is_empty() { fallback } else { value });
            }
            // Not a `${digits:-…}` form: the regex would not match, so the text stands.
            _ => out.push_str(tail.get(..=close).unwrap_or_default()),
        }
        rest = tail.get(close.saturating_add(1)..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// The `\$(\d+)` pass of [`substitute_args`].
fn replace_bare_positionals(input: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(at) = rest.find('$') {
        let (head, tail) = rest.split_at(at);
        out.push_str(head);
        let digits: String = tail
            .chars()
            .skip(1)
            .take_while(char::is_ascii_digit)
            .collect();
        if digits.is_empty() {
            out.push('$');
            rest = tail.get(1..).unwrap_or_default();
            continue;
        }
        let index = digits.parse::<usize>().unwrap_or(0);
        out.push_str(positional(args, index));
        rest = tail.get(digits.len().saturating_add(1)..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// pi `parseRuntimeOptions`' return (`:174-211`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// Every non-flag word, in order — the positional arguments `$1`/`$ARGUMENTS` substitute from.
    pub args: Vec<String>,
    /// `--subagent <name>` / `--subagent=<name>` / `--subagent:<name>`: replaces the recipe's own
    /// persona.
    pub agent_override: Option<String>,
    /// `--fork`.
    pub fork: bool,
    /// `--fresh`.
    pub fresh: bool,
    /// `--worktree`.
    pub worktree: bool,
    /// `--bg` or `--async`.
    pub bg: bool,
}

/// pi `parseRuntimeOptions` (`:174-211`). A trailing bare `--subagent` with no following word
/// consumes nothing and leaves `agent_override` unset — upstream's `words[++i]` yields `undefined`
/// there, and `??` in `workflowParams` then falls back to the recipe's own agent.
#[must_use]
pub fn parse_runtime_options(words: &[String]) -> RuntimeOptions {
    let mut out = RuntimeOptions::default();
    let mut i = 0usize;
    while let Some(word) = words.get(i) {
        i = i.saturating_add(1);
        match word.as_str() {
            "--fork" => out.fork = true,
            "--fresh" => out.fresh = true,
            "--worktree" => out.worktree = true,
            "--bg" | "--async" => out.bg = true,
            "--subagent" => {
                out.agent_override = words.get(i).cloned();
                i = i.saturating_add(1);
            }
            other => {
                match other
                    .strip_prefix("--subagent=")
                    .or_else(|| other.strip_prefix("--subagent:"))
                    .filter(|v| !v.is_empty())
                {
                    Some(name) => out.agent_override = Some(name.to_string()),
                    None => out.args.push(other.to_string()),
                }
            }
        }
    }
    out
}

/// pi `splitChainDeclaration` (`:213-217`): everything before the first ` -- ` is the chain
/// declaration, everything after is the argument text. No delimiter means "all declaration".
#[must_use]
pub fn split_chain_declaration(input: &str) -> (String, String) {
    match input.find(" -- ") {
        None => (input.trim().to_string(), String::new()),
        Some(at) => (
            input.get(..at).unwrap_or_default().trim().to_string(),
            input
                .get(at.saturating_add(4)..)
                .unwrap_or_default()
                .trim()
                .to_string(),
        ),
    }
}

/// pi `splitPromptChain` (`:219-221`): split on the literal ` -> `, trim, drop empties.
#[must_use]
pub fn split_prompt_chain(input: &str) -> Vec<String> {
    input
        .split(" -> ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

// =================================================================================================
// Lowering a recipe to a run (workflowParams / workflowChainStep)
// =================================================================================================

/// pi `workflowParams`' output (`:223-238`) — the `SubagentParamsLike` a recipe lowers to, in the
/// subset cyrup's dispatch surface consumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowRun {
    /// `runtime.agentOverride ?? workflow.agent`.
    pub agent: String,
    /// `substituteArgs(workflow.body, args).trim()`.
    pub task: String,
    /// `runtime.fork ? "fork" : runtime.fresh ? "fresh" : workflow.context`.
    pub context: Option<ContextMode>,
    /// `workflow.model`.
    pub model: Option<String>,
    /// `workflow.skill`, normalized (see [`PromptWorkflow::skills`]).
    pub skills: Option<Vec<String>>,
    /// `workflow.cwd`.
    pub cwd: Option<String>,
    /// `runtime.worktree || workflow.worktree`.
    pub worktree: bool,
    /// `runtime.bg` → upstream's `async: true`.
    pub background: bool,
}

/// pi `workflowParams` (`:223-238`). `clarify: false` and `agentScope: "both"` are constants
/// upstream sets on every dispatch; cyrup's `/run` slash path already pins `AgentReadScope::Both`
/// for the same reason and has no clarify surface, so neither becomes a field here.
#[must_use]
pub fn workflow_params(
    workflow: &PromptWorkflow,
    args: &[String],
    runtime: &RuntimeOptions,
) -> WorkflowRun {
    WorkflowRun {
        agent: runtime.agent_override.clone().unwrap_or_else(|| workflow.agent.clone()),
        task: substitute_args(&workflow.body, args).trim().to_string(),
        context: if runtime.fork {
            Some(ContextMode::Fork)
        } else if runtime.fresh {
            Some(ContextMode::Fresh)
        } else {
            workflow.context
        },
        model: workflow.model.clone(),
        skills: workflow.skills.clone(),
        cwd: workflow.cwd.clone(),
        worktree: runtime.worktree || workflow.worktree,
        background: runtime.bg,
    }
}

/// pi `workflowChainStep` (`:240-249`): the same lowering, kept as a distinct name because upstream
/// narrows the result to a `ChainStep` (agent/task/model/skill/cwd) — the `context`/`worktree`/
/// `async` fields are deliberately NOT carried per step; they are decided once for the whole chain
/// by the caller's own runtime options (`:293`,`:324`).
#[must_use]
pub fn workflow_chain_step(
    workflow: &PromptWorkflow,
    args: &[String],
    runtime: &RuntimeOptions,
) -> WorkflowRun {
    let params = workflow_params(workflow, args, runtime);
    WorkflowRun {
        // `params.agent ?? "delegate"` (`:243`) — `workflow_params` already guarantees a name, but
        // the fallback is kept so the two functions cannot disagree if that ever changes.
        agent: if params.agent.is_empty() {
            DEFAULT_WORKFLOW_AGENT.to_string()
        } else {
            params.agent
        },
        context: None,
        worktree: false,
        background: false,
        ..params
    }
}

/// Expand a `chain:` declaration (or a `/chain-prompts` declaration) into its ordered steps,
/// resolving every name against `workflows` (`:288-292` / `:319-323`). The `Err` string is
/// upstream's exact thrown message, which its handler surfaces via `ctx.ui.notify(…, "error")`.
///
/// `chain_owner` names the recipe whose `chain:` field is being expanded, which upstream includes in
/// the error (`Unknown prompt workflow in chain '<name>': <step>`, `:290`); `None` selects the
/// `/chain-prompts` wording (`Unknown prompt workflow: <step>`, `:321`).
pub fn build_chain_steps(
    workflows: &[PromptWorkflow],
    names: &[String],
    args: &[String],
    runtime: &RuntimeOptions,
    chain_owner: Option<&str>,
) -> Result<Vec<WorkflowRun>, String> {
    names
        .iter()
        .map(|step_name| {
            find_workflow(workflows, step_name)
                .ok_or_else(|| match chain_owner {
                    Some(owner) => {
                        format!("Unknown prompt workflow in chain '{owner}': {step_name}")
                    }
                    None => format!("Unknown prompt workflow: {step_name}"),
                })
                .map(|step| workflow_chain_step(step, args, runtime))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

    use super::*;

    fn words(input: &str) -> Vec<String> {
        shell_words(input)
    }

    /// The FIVE bundled recipes are reachable through the real discovery entry point — not through
    /// `bundled_prompt_files()` directly. This is the package tier of `promptDirs` (`:43`).
    /// (Seven until `83b9872` deleted `parallel-context-build`/`parallel-handoff-plan` alongside the
    /// `planner`/`context-builder` roles they drove.)
    #[test]
    fn the_bundled_recipes_are_discoverable_by_name() {
        let empty = tempfile::tempdir().unwrap();
        let workflows = discover_prompt_workflows(empty.path());
        let names: Vec<&str> = workflows.iter().map(|w| w.name.as_str()).collect();
        for expected in [
            "gather-context-and-clarify",
            "parallel-cleanup",
            "parallel-research",
            "parallel-review",
            "review-loop",
        ] {
            assert!(names.contains(&expected), "expected {expected:?} in {names:?}");
        }
        // Sorted by name (`:125`).
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        // Every one carries a real persona and a non-empty body to substitute into.
        for w in &workflows {
            assert!(!w.agent.is_empty(), "{} has no agent", w.name);
            assert!(!w.body.trim().is_empty(), "{} has an empty body", w.name);
        }
    }

    /// A project recipe SHADOWS the bundled one of the same name (`workflows.set`, `:123`, with the
    /// project dir read last).
    #[test]
    fn a_project_recipe_overrides_the_bundled_one_of_the_same_name() {
        let dir = tempfile::tempdir().unwrap();
        let prompts = dir.path().join(".cyrup").join("prompts");
        std::fs::create_dir_all(&prompts).unwrap();
        std::fs::write(
            prompts.join("parallel-review.md"),
            "---\ndescription: project override\nsubagent: reviewer\n---\nReview $1\n",
        )
        .unwrap();

        let workflows = discover_prompt_workflows(dir.path());
        let found = find_workflow(&workflows, "parallel-review").expect("shadowed entry present");
        assert_eq!(found.description, "project override");
        assert_eq!(found.agent, "reviewer");
        assert_eq!(
            workflows.iter().filter(|w| w.name == "parallel-review").count(),
            1,
            "shadowing replaces, never duplicates"
        );
    }

    /// A recipe named after a reserved command is dropped at load (`:98`) — `prompts/run.md` must
    /// not become a second `/run`.
    #[test]
    fn a_reserved_name_is_never_registered_as_a_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let prompts = dir.path().join(".cyrup").join("prompts");
        std::fs::create_dir_all(&prompts).unwrap();
        for reserved in RESERVED_COMMAND_NAMES {
            std::fs::write(prompts.join(format!("{reserved}.md")), "Body\n").unwrap();
        }
        let workflows = discover_prompt_workflows(dir.path());
        for reserved in RESERVED_COMMAND_NAMES {
            assert!(
                find_workflow(&workflows, reserved).is_none(),
                "{reserved} must be refused"
            );
        }
    }

    #[test]
    fn shell_words_handles_quotes_and_escapes() {
        assert_eq!(words("a  b\tc"), vec!["a", "b", "c"]);
        assert_eq!(words(r#"'one two' three"#), vec!["one two", "three"]);
        assert_eq!(words(r#""a b" 'c d'"#), vec!["a b", "c d"]);
        assert_eq!(words(r"a\ b"), vec!["a b"], "a backslash escapes the space");
        assert_eq!(words("'unterminated"), vec!["unterminated"]);
    }

    /// The two positional forms are NOT the same rule: `${1:-x}` uses `||` (empty takes the
    /// fallback), `$1` uses `??` (absent becomes empty).
    #[test]
    fn substitution_matches_pis_four_passes() {
        let args = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(substitute_args("all: $ARGUMENTS", &args), "all: alpha beta");
        assert_eq!(substitute_args("all: $@", &args), "all: alpha beta");
        assert_eq!(substitute_args("$1/$2/$3", &args), "alpha/beta/");
        assert_eq!(substitute_args("${3:-fallback}", &args), "fallback");
        assert_eq!(substitute_args("${1:-fallback}", &args), "alpha");
        assert_eq!(
            substitute_args("${1:-fallback}", &["".to_string()]),
            "fallback",
            "an EMPTY supplied argument still takes the fallback (JS `||`)"
        );
        assert_eq!(substitute_args("cost: $5.00", &args), "cost: .00", "`$5` is a positional");
        assert_eq!(substitute_args("plain $ text", &args), "plain $ text");
    }

    #[test]
    fn runtime_options_parse_every_flag_form() {
        let opts = parse_runtime_options(&words(
            "fix --fork --worktree --bg --subagent worker the thing",
        ));
        assert_eq!(opts.args, vec!["fix", "the", "thing"]);
        assert_eq!(opts.agent_override.as_deref(), Some("worker"));
        assert!(opts.fork && opts.worktree && opts.bg);
        assert!(!opts.fresh);

        assert_eq!(
            parse_runtime_options(&words("--subagent=worker")).agent_override.as_deref(),
            Some("worker")
        );
        assert_eq!(
            parse_runtime_options(&words("--subagent:worker")).agent_override.as_deref(),
            Some("worker")
        );
        assert!(parse_runtime_options(&words("--async")).bg, "--async is an alias of --bg");
        assert!(
            parse_runtime_options(&words("x --subagent")).agent_override.is_none(),
            "a trailing bare --subagent leaves the recipe's own agent in place"
        );
    }

    #[test]
    fn chain_declaration_and_arrow_splitting() {
        let (decl, args) = split_chain_declaration("a -> b -- do the thing");
        assert_eq!(decl, "a -> b");
        assert_eq!(args, "do the thing");
        let (decl, args) = split_chain_declaration("  a -> b  ");
        assert_eq!(decl, "a -> b");
        assert_eq!(args, "");
        assert_eq!(split_prompt_chain("a -> b -> c"), vec!["a", "b", "c"]);
        assert_eq!(split_prompt_chain(" -> "), Vec::<String>::new());
    }

    fn workflow(name: &str) -> PromptWorkflow {
        PromptWorkflow {
            name: name.to_string(),
            description: "d".to_string(),
            body: "Do $1".to_string(),
            file_path: PathBuf::from("/tmp/x.md"),
            agent: "planner".to_string(),
            context: Some(ContextMode::Fork),
            model: Some("m".to_string()),
            skills: Some(vec!["s".to_string()]),
            cwd: Some("sub".to_string()),
            worktree: false,
            chain: None,
        }
    }

    #[test]
    fn runtime_flags_override_the_recipes_own_context_and_agent() {
        let w = workflow("w");
        let args = vec!["it".to_string()];
        let plain = workflow_params(&w, &args, &RuntimeOptions::default());
        assert_eq!(plain.task, "Do it");
        assert_eq!(plain.agent, "planner");
        assert_eq!(plain.context, Some(ContextMode::Fork), "the recipe's own context stands");

        let fresh = RuntimeOptions { fresh: true, ..RuntimeOptions::default() };
        assert_eq!(workflow_params(&w, &args, &fresh).context, Some(ContextMode::Fresh));

        let over = RuntimeOptions {
            agent_override: Some("other".to_string()),
            worktree: true,
            bg: true,
            ..RuntimeOptions::default()
        };
        let run = workflow_params(&w, &args, &over);
        assert_eq!(run.agent, "other");
        assert!(run.worktree && run.background);
    }

    /// A chain STEP drops context/worktree/async — those are chain-wide, decided once by the caller
    /// (`:242-248`).
    #[test]
    fn a_chain_step_carries_no_per_step_context_or_async() {
        let w = workflow("w");
        let runtime = RuntimeOptions { fork: true, bg: true, worktree: true, ..RuntimeOptions::default() };
        let step = workflow_chain_step(&w, &[], &runtime);
        assert_eq!(step.context, None);
        assert!(!step.worktree);
        assert!(!step.background);
        assert_eq!(step.model.as_deref(), Some("m"), "model/skill/cwd DO survive");
        assert_eq!(step.cwd.as_deref(), Some("sub"));
    }

    #[test]
    fn an_unknown_chain_step_reports_pis_exact_message() {
        let workflows = vec![workflow("a")];
        let names = vec!["a".to_string(), "missing".to_string()];
        let err = build_chain_steps(&workflows, &names, &[], &RuntimeOptions::default(), Some("outer"))
            .expect_err("an unknown step must fail the whole expansion");
        assert_eq!(err, "Unknown prompt workflow in chain 'outer': missing");
        let err = build_chain_steps(&workflows, &names, &[], &RuntimeOptions::default(), None)
            .expect_err("the /chain-prompts wording differs");
        assert_eq!(err, "Unknown prompt workflow: missing");
    }

    #[test]
    fn the_empty_list_renders_pis_exact_sentence() {
        assert_eq!(
            format_workflow_list(&[]),
            "No prompt workflows found in package, user, or project prompts."
        );
        let rendered = format_workflow_list(&[workflow("w")]);
        assert!(rendered.starts_with("Prompt workflows:\n- w: d (/tmp/x.md)"), "{rendered}");
    }

    #[test]
    fn skill_frontmatter_normalizes_to_the_override_tristate() {
        assert_eq!(parse_skill(None), None);
        assert_eq!(parse_skill(Some("false")), Some(Vec::new()));
        assert_eq!(parse_skill(Some("one")), Some(vec!["one".to_string()]));
        assert_eq!(
            parse_skill(Some("one, two")),
            Some(vec!["one".to_string(), "two".to_string()])
        );
    }
}
