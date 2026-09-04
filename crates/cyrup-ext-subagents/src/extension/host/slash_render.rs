//! Free helpers backing the [`crate::extension::SubagentsExtension::dispatch_slash`] arms: chain description,
//! fork-context application and result rendering.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::SubagentError;
use crate::exec::ResolvedAgentPersona;
use crate::fork_context::{
    ContextMode, ContextRequest, ForkContextResolver, resolve_effective_context,
};
use crate::spawn::chain_graph::{GroupStepResult, RunnerStep, SingleStepSpec, StepResult};

/// Every agent name a [`RunnerStep`] graph will dispatch, in walk order — a single step's own
/// agent, each parallel-group child's agent, and a dynamic group's per-item template agent. This is
/// the plan-time persona resolver's input set: [`crate::extension::SubagentsExtension::run_or_background_chain`]
/// resolves the whole set via [`crate::extension::SubagentExecutor::resolve_plan_personas`] before any child
/// is spawned (T0.1/C13 plan-time resolution + upfront agent-name validation).
pub(crate) fn plan_step_agent_names(graph: &[RunnerStep]) -> Vec<String> {
    let mut names = Vec::new();
    for step in graph {
        match step {
            RunnerStep::SingleStep(spec) => names.push(spec.agent.clone()),
            RunnerStep::ParallelGroup(group) => {
                names.extend(group.steps.iter().map(|spec| spec.agent.clone()));
            }
            RunnerStep::DynamicGroup(dynamic) => names.push(dynamic.template.agent.clone()),
            // A root-attachment step names no agent to discover/resolve at plan time — its agent is
            // whatever the ALREADY-launched target run resolved for itself, read back from the
            // target's result at poll time (R-SA-097). Contributing its display name here would make
            // plan-time persona resolution demand an agent the attaching chain never spawns.
            RunnerStep::ImportAsyncRoot(_) => {}
        }
    }
    names
}

/// The graph's first step's first task text — the fallback `{task}` value when the call site
/// supplied no explicit top-level task (pi `originalTask = params.task ?? firstStepFirstTask`,
/// `chain-execution.ts:493-497`). A single step's own task, a parallel group's first child's task, or
/// a dynamic group's per-item template task; an `ImportAsyncRoot`-led graph (or an empty graph) has
/// no authored task text, yielding the empty string (so `{task}` → `""`, matching pi's empty case).
pub(crate) fn first_step_task(graph: &[RunnerStep]) -> String {
    graph
        .iter()
        .find_map(|step| match step {
            RunnerStep::SingleStep(spec) => Some(spec.task.clone()),
            RunnerStep::ParallelGroup(group) => group.steps.first().map(|spec| spec.task.clone()),
            RunnerStep::DynamicGroup(dynamic) => Some(dynamic.template.task.clone()),
            RunnerStep::ImportAsyncRoot(_) => None,
        })
        .unwrap_or_default()
}

/// pi `formatAsyncStartedMessage` (`async-execution.ts:261-267`): the mode-specific `headline`
/// followed verbatim by the fixed four-line detached-run guidance (blank line, then three
/// instruction lines), joined with `"\n"` exactly as pi's `.join("\n")` does.
pub(crate) fn format_async_started_message(headline: &str) -> String {
    [
        headline,
        "",
        "The async run is detached. Do not run sleep timers or polling loops just to wait for it.",
        "If you have independent work, continue that work. If you have nothing else to do until \
         the async result arrives, end your turn now; Pi will deliver the completion when the run \
         finishes.",
        "Use subagent({ action: \"status\", id: \"...\" }) when you need the current status/result, \
         or to inspect a blocked/stale run. Do not poll just to wait.",
    ]
    .join("\n")
}

/// One chain-step's display descriptor for the `chainDesc` join (pi `async-execution.ts:1183-1197`):
/// a sequential step is its bare agent name, a static parallel group is `[a+b]`, a dynamic group is
/// `expand:agent`, and a root-attachment step is its (fallback) display agent name.
fn describe_chain_step(step: &RunnerStep) -> String {
    match step {
        RunnerStep::SingleStep(spec) => spec.agent.clone(),
        RunnerStep::ParallelGroup(group) => format!(
            "[{}]",
            group
                .steps
                .iter()
                .map(|spec| spec.agent.as_str())
                .collect::<Vec<_>>()
                .join("+")
        ),
        RunnerStep::DynamicGroup(dynamic) => format!("expand:{}", dynamic.template.agent),
        RunnerStep::ImportAsyncRoot(spec) => spec.agent.clone(),
    }
}

/// The full `chainDesc` pi joins with `" -> "` (`async-execution.ts:1183-1197`) to build the async-start
/// headline for a CHAIN/PARALLEL run.
pub(crate) fn describe_chain(graph: &[RunnerStep]) -> String {
    graph
        .iter()
        .map(describe_chain_step)
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// Resolve every step's effective fork-context and, for each forking step, mint its OWN per-index
/// branch session file — the Tier-2 fork default-mode + per-index-branch wire-up (pi
/// `resolveAgentDefaultContextPolicy` + `preflightForkSessionsForStaticTasks`,
/// `subagent-executor.ts:2285-2324`). Two behaviors this replaces the old single-shared-branch
/// `apply_default_context` with:
///
/// 1. **Fork default-mode.** An OMITTED call-site `context` (`None`) no longer forces `Fresh` on
///    every step; each step independently falls back to ITS OWN agent's persona `default_context`
///    via [`resolve_effective_context`] (`personas[agent].default_context`). An explicit call-site
///    `context` still wins for every step; a step's own explicit `context` wins over both (R-SA-138).
/// 2. **Per-index branch.** Rather than resolving ONE fork branch (index 0) and splicing that same
///    session file into every step, each FORKING step resolves its own branch at its own flat step
///    index off the SINGLE shared `resolver` (whose per-index cache mints a distinct branch per
///    index) — so two sibling parallel tasks that both fork get two DISTINCT branch session files.
///    The flat index increments for EVERY step (matching pi's `preflightForkSessionsForStaticTasks`
///    flat-index walk), forking or not, so indices are stable and never collide.
///
/// A step that already carries an explicit `session_file` keeps it (never re-branched). Returns the
/// (mutated) graph plus the FIRST forking step's branch path, used only as the run-level resume
/// session metadata (`RunnerConfig::session_file`); it is never spliced onto any step here.
///
/// Fails hard (R-SA-137/DI-SA-2) if any forking step's branch cannot be resolved (unpersisted parent,
/// no leaf) — resolving every step up front, before any child is spawned, so a later step's fork
/// failure aborts the whole batch rather than leaving earlier children already running.
pub(crate) async fn apply_fork_contexts(
    resolver: &ForkContextResolver,
    call_site_context: Option<ContextRequest>,
    // SUBA-079 — the resolved `subagents.defaultSubagentContext` rung, validated by the caller
    // (the only side holding the live extension config).
    config_default: Option<ContextMode>,
    personas: &BTreeMap<String, ResolvedAgentPersona>,
    mut graph: Vec<RunnerStep>,
) -> Result<(Vec<RunnerStep>, Option<PathBuf>), SubagentError> {
    let mut flat_index: u32 = 0;
    let mut first_session_file: Option<PathBuf> = None;
    for step in &mut graph {
        match step {
            RunnerStep::SingleStep(spec) => {
                resolve_step_fork_context(
                    resolver,
                    call_site_context,
                    config_default,
                    personas,
                    spec,
                    &mut flat_index,
                    &mut first_session_file,
                )
                .await?;
            }
            RunnerStep::ParallelGroup(group) => {
                for spec in &mut group.steps {
                    resolve_step_fork_context(
                        resolver,
                        call_site_context,
                        config_default,
                        personas,
                        spec,
                        &mut flat_index,
                        &mut first_session_file,
                    )
                    .await?;
                }
            }
            RunnerStep::DynamicGroup(dynamic) => {
                resolve_step_fork_context(
                    resolver,
                    call_site_context,
                    config_default,
                    personas,
                    &mut dynamic.template,
                    &mut flat_index,
                    &mut first_session_file,
                )
                .await?;
            }
            // A root-attachment step carries no fork-vs-fresh context of its own: it imports another,
            // already-completed run's result rather than spawning a fresh child whose session context
            // this resolution would seed. Left untouched (and does not consume a flat index).
            RunnerStep::ImportAsyncRoot(_) => {}
        }
    }
    Ok((graph, first_session_file))
}

/// Resolve one step's effective context (per-step explicit > call-site > config default > persona
/// default > `Fresh`)
/// and, when it resolves to `Fork` and the step has no explicit session file yet, mint its own
/// per-`*flat_index*` branch off `resolver`. Always advances `*flat_index*` by one so sibling steps
/// never share an index (and therefore never a branch). See [`apply_fork_contexts`].
async fn resolve_step_fork_context(
    resolver: &ForkContextResolver,
    call_site_context: Option<ContextRequest>,
    config_default: Option<ContextMode>,
    personas: &BTreeMap<String, ResolvedAgentPersona>,
    spec: &mut SingleStepSpec,
    flat_index: &mut u32,
    first_session_file: &mut Option<PathBuf>,
) -> Result<(), SubagentError> {
    let index = *flat_index;
    *flat_index += 1;

    // Precedence: a step's OWN explicit `context` wins outright; else the full ladder.
    //
    // SUBA-079: a step's `context` is already a RESOLVED [`ContextMode`], so it short-circuits
    // ahead of the policy rather than being widened into a request — an explicit per-step mode is
    // strict for the same reason an explicit call-site one is.
    let persona_default = personas.get(&spec.agent).and_then(|p| p.default_context);
    let effective = match spec.context {
        Some(mode) => mode,
        Option::None => {
            let can_prefer_fork = match call_site_context {
                Option::None => resolver.can_prefer_fork().await,
                Some(_) => false,
            };
            resolve_effective_context(
                call_site_context,
                &spec.agent,
                persona_default,
                config_default,
                can_prefer_fork,
            )?
        }
    };
    spec.context = Some(effective);

    if effective == ContextMode::Fork {
        if spec.session_file.is_none() {
            // SUBA-075: upstream's `?? true` fallback — a chain/parallel step's resolved model
            // ladder is not in hand here, and `SingleStepSpec` carries only the session file
            // forward, so this path takes the conservative arm (see `background.rs`'s note).
            let fork_context = resolver.resolve(ContextMode::Fork, index, true).await?;
            spec.session_file = fork_context.session_file_path.clone();
            if first_session_file.is_none() {
                *first_session_file = fork_context.session_file_path;
            }
        } else if first_session_file.is_none() {
            first_session_file.clone_from(&spec.session_file);
        }
    }
    Ok(())
}

/// `/run-chain`'s task-seeding rule (see this command's own doc note in `dispatch_slash`): splice
/// `task` into the first element's first task only, leaving every later step's saved task text
/// verbatim.
pub(crate) fn seed_first_step_task(mut steps: Vec<RunnerStep>, task: &str) -> Vec<RunnerStep> {
    if task.is_empty() {
        return steps;
    }
    if let Some(first) = steps.first_mut() {
        match first {
            RunnerStep::SingleStep(spec) => spec.task = task.to_string(),
            RunnerStep::ParallelGroup(group) => {
                if let Some(first_task) = group.steps.first_mut() {
                    first_task.task = task.to_string();
                }
            }
            RunnerStep::DynamicGroup(_) => {
                // A `DynamicGroup` has no single fixed task to overwrite (its per-item tasks come
                // from `template` instantiated once per resolved array element) — left as saved.
            }
            RunnerStep::ImportAsyncRoot(_) => {
                // A root-attachment step's "task" is fixed by the target run it imports; there is no
                // free task text to seed (R-SA-097) — left as saved.
            }
        }
    }
    steps
}

/// Render [`StepResult`]s from a foreground `/chain`/`/parallel`/`/run-chain` run as human-readable
/// text — one line per step, in chain order (R-SA-051's ordering guarantee, restated at this
/// command's own text-rendering layer).
pub(crate) fn render_chain_results(
    results: &[StepResult],
    is_group: &[bool],
    groups: &[GroupStepResult],
) -> String {
    let mut out = String::new();
    let mut group_cursor = 0usize;
    for (i, result) in results.iter().enumerate() {
        let step_is_group = is_group.get(i).copied().unwrap_or(false);
        if step_is_group {
            // A group step's own aggregate `StepResult::final_output` is always `None` by
            // construction (`chain_graph::collapse_fan_out`'s own doc: the aggregate carries only
            // a `structured_output` array, never a collapsed text field) — render each fanned-out
            // child's own text output instead, in the SAME position-indexed order `run_bounded`
            // guarantees (R-SA-051), so a `/parallel` caller can see every child's real output,
            // not just an aggregate "ok"/"failed" line with no text at all.
            let group = groups.get(group_cursor);
            group_cursor += 1;
            if result.success {
                out.push_str(&format!("step {}: ok (parallel group)\n", i + 1));
            } else {
                let err = result
                    .error
                    .clone()
                    .unwrap_or_else(|| "unknown error".to_string());
                out.push_str(&format!(
                    "step {}: FAILED (parallel group) — {err}\n",
                    i + 1
                ));
            }
            if let Some(group) = group {
                for (child_i, child) in group.children.iter().enumerate() {
                    match child {
                        Some(child_result) if child_result.success => {
                            let text = child_result
                                .final_output
                                .clone()
                                .unwrap_or_else(|| "(no text output)".to_string());
                            out.push_str(&format!("  child {}: ok\n  {text}\n", child_i + 1));
                        }
                        Some(child_result) => {
                            let err = child_result
                                .error
                                .clone()
                                .unwrap_or_else(|| "unknown error".to_string());
                            out.push_str(&format!("  child {}: FAILED — {err}\n", child_i + 1));
                        }
                        None => {
                            out.push_str(&format!("  child {}: skipped\n", child_i + 1));
                        }
                    }
                }
            }
        } else if result.success {
            let text = result
                .final_output
                .clone()
                .unwrap_or_else(|| "(no text output)".to_string());
            out.push_str(&format!("step {}: ok\n{text}\n", i + 1));
        } else {
            let err = result
                .error
                .clone()
                .unwrap_or_else(|| "unknown error".to_string());
            out.push_str(&format!("step {}: FAILED — {err}\n", i + 1));
        }
        if i + 1 != results.len() {
            out.push('\n');
        }
    }
    if out.is_empty() {
        out.push_str("(chain produced no step results)");
    }
    out
}

/// pi `BUILTIN_AGENT_NAMES` (`agents.ts:38-46` @ v0.43.0): the fixed 7 builtin persona NAMES, in
/// pi's declared order. `/subagents-models`' all-agents view (and the single-agent name gate) walks
/// EXACTLY this static list — not whatever discovery happened to find — so a name discovery didn't
/// resolve renders its own "missing"/"not found" row rather than silently shrinking the report.
///
/// See [`crate::discovery::management::BUILTIN_AGENT_NAMES`] for why `planner`/`context-builder`
/// are gone and why `advisor` is here despite shipping no `advisor.md` (it is an `oracle` alias).
pub(crate) const BUILTIN_AGENT_NAMES: [&str; 7] = [
    "advisor",
    "delegate",
    "oracle",
    "researcher",
    "reviewer",
    "scout",
    "worker",
];

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::extension::testsupport::fork_assistant_msg;
    use crate::extension::testsupport::fork_user_msg;
    use crate::spawn::chain_graph::ParallelGroupSpec;
    use std::path::Path;
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    /// pi `formatAsyncStartedMessage` (`async-execution.ts:261-267`): the mode-specific headline
    /// followed by the fixed 4-line detached-run guidance, `"\n"`-joined verbatim. Before this fix,
    /// an async-start tool result was the single flat sentence "Background subagent run started:
    /// {run_id}. Use the status/interrupt management actions to check on it later; do not poll in a
    /// tight loop." — this exact multi-line shape did not exist.
    #[test]
    fn format_async_started_message_matches_pis_fixed_four_line_guidance() {
        let msg = format_async_started_message("Async: worker [run00001]");
        assert_eq!(
            msg,
            "Async: worker [run00001]\n\
             \n\
             The async run is detached. Do not run sleep timers or polling loops just to wait for it.\n\
             If you have independent work, continue that work. If you have nothing else to do until \
             the async result arrives, end your turn now; Pi will deliver the completion when the run \
             finishes.\n\
             Use subagent({ action: \"status\", id: \"...\" }) when you need the current status/result, \
             or to inspect a blocked/stale run. Do not poll just to wait."
        );
    }

    /// pi's `chainDesc` (`async-execution.ts:1183-1197`): sequential steps joined by `" -> "`, a static
    /// parallel group rendered as `[a+b]`. Before this fix, the tool's async-start headline never
    /// described the chain shape at all — `describe_chain` did not exist.
    #[test]
    fn describe_chain_joins_sequential_steps_and_brackets_parallel_groups() {
        let graph = vec![
            RunnerStep::SingleStep(fork_test_step("a")),
            RunnerStep::ParallelGroup(ParallelGroupSpec {
                steps: vec![fork_test_step("b"), fork_test_step("c")],
                concurrency: 2,
                fail_fast: false,
                worktree: false,
            }),
        ];
        assert_eq!(describe_chain(&graph), "a -> [b+c]");
    }

    /// Build a REAL persisted parent session (tempdir-backed on-disk JSONL, never mocked — mirrors
    /// `fork_context.rs`'s own test setup) and a [`ForkContextResolver`] over it, so `Fork` requests
    /// actually branch a genuine new session file on disk. Returns the tempdir so the caller keeps it
    /// alive for the test's duration.
    async fn persisted_fork_resolver() -> (ForkContextResolver, tempfile::TempDir) {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = root.path().join("proj");
        let layout = cyrup_session::SessionLayout::new(root.path().to_path_buf(), cwd.clone());
        let mut parent = cyrup_session::SessionManager::create(
            &cwd,
            &layout,
            cyrup_session::NewSessionOpts::default(),
        )
        .expect("create persisted parent session");
        parent
            .append_message(fork_user_msg("hello"))
            .expect("append user");
        parent
            .append_message(fork_assistant_msg("hi there"))
            .expect("append assistant");
        let manager = Arc::new(AsyncMutex::new(parent));
        let resolver = ForkContextResolver::new(manager, layout);
        (resolver, root)
    }

    fn fork_test_step(agent: &str) -> SingleStepSpec {
        SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: agent.to_string(),
            task: format!("do {agent}"),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        }
    }

    fn persona_with_default_context(
        name: &str,
        default_context: Option<ContextMode>,
    ) -> ResolvedAgentPersona {
        ResolvedAgentPersona {
            name: name.to_string(),
            model: None,
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: crate::discovery::types::SystemPromptMode::Replace,
            system_prompt_body: String::new(),
            tools: None,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            exclude_tools: Vec::new(),
            allow_nested_subagents: None,
            output: None,
            inherit_project_context: false,
            inherit_skills: false,
            skills: Vec::new(),
            completion_guard: None,
            max_subagent_depth: None,
            default_context,
            memory: None,
            tool_budget: None,
            runner: None,
            acceptance_role: None,
            default_acceptance: None,
        }
    }

    fn single_step_of(step: &RunnerStep) -> &SingleStepSpec {
        match step {
            RunnerStep::SingleStep(spec) => spec,
            other => panic!("expected a SingleStep, got {other:?}"),
        }
    }

    /// (a) An OMITTED call-site `context` (`None`) resolves EACH step to its own agent's persona
    /// `default_context` — a `fork`-defaulting agent forks, a `fresh`-defaulting agent stays fresh —
    /// rather than the pre-Tier-2 forced-`Fresh` collapse. Mirrors pi's
    /// `resolveAgentDefaultContextPolicy` (`subagent-executor.ts:1875-1891`).
    #[tokio::test]
    async fn omitted_call_site_context_falls_back_to_each_agents_persona_default() {
        let (resolver, _root) = persisted_fork_resolver().await;
        let personas: BTreeMap<String, ResolvedAgentPersona> = [
            (
                "planner".to_string(),
                persona_with_default_context("planner", Some(ContextMode::Fork)),
            ),
            (
                "scout".to_string(),
                persona_with_default_context("scout", Some(ContextMode::Fresh)),
            ),
        ]
        .into_iter()
        .collect();
        let graph = vec![
            RunnerStep::SingleStep(fork_test_step("planner")),
            RunnerStep::SingleStep(fork_test_step("scout")),
        ];

        // call_site_context = None (omitted).
        let (graph, first_session) = apply_fork_contexts(&resolver, None, None, &personas, graph)
            .await
            .expect("fork contexts resolve against a persisted parent");

        let planner = single_step_of(&graph[0]);
        let scout = single_step_of(&graph[1]);
        assert_eq!(
            planner.context,
            Some(ContextMode::Fork),
            "planner's persona default_context (fork) must be honored when the call site omits context"
        );
        assert!(
            planner.session_file.as_deref().is_some_and(Path::exists),
            "a fork step must receive a real, on-disk branched session file"
        );
        assert_eq!(
            scout.context,
            Some(ContextMode::Fresh),
            "scout's persona default_context (fresh) must be honored independently — one sibling's \
             default must not leak into another's"
        );
        assert!(
            scout.session_file.is_none(),
            "a fresh step must carry no branched session file"
        );
        assert_eq!(
            first_session, planner.session_file,
            "the run-level resume session is the first forking step's branch"
        );
    }

    /// (a) Two parallel fork tasks get two DISTINCT branch session files (per-index branch), not one
    /// shared branch. Mirrors pi's per-index `sessionFileForTask(agent, index)`
    /// (`preflightForkSessionsForStaticTasks`, `subagent-executor.ts:2285-2324`).
    #[tokio::test]
    async fn two_parallel_fork_tasks_get_two_distinct_branch_session_files() {
        let (resolver, _root) = persisted_fork_resolver().await;
        let personas: BTreeMap<String, ResolvedAgentPersona> = [(
            "planner".to_string(),
            persona_with_default_context("planner", None),
        )]
        .into_iter()
        .collect();
        // Two sibling forking tasks (same agent) in one parallel group; explicit call-site fork.
        let group = RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: vec![fork_test_step("planner"), fork_test_step("planner")],
            concurrency: 4,
            fail_fast: false,
            worktree: false,
        });

        let (graph, _first) = apply_fork_contexts(
            &resolver,
            Some(ContextRequest::Fork),
            None,
            &personas,
            vec![group],
        )
        .await
        .expect("both parallel fork tasks resolve");

        let steps = match &graph[0] {
            RunnerStep::ParallelGroup(g) => &g.steps,
            other => panic!("expected a ParallelGroup, got {other:?}"),
        };
        let first = steps[0]
            .session_file
            .clone()
            .expect("parallel task 0 forks and gets a branch");
        let second = steps[1]
            .session_file
            .clone()
            .expect("parallel task 1 forks and gets a branch");
        assert_ne!(
            first, second,
            "two parallel fork tasks must get two DISTINCT branch session files, not one shared branch"
        );
        assert!(
            first.exists() && second.exists(),
            "both branch files must exist on disk"
        );
    }
}
