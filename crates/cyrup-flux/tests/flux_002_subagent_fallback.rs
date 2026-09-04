//! FLUX-002 — the four multi-task templates name the `subagent` tool, which is default-OFF, while
//! Flux itself is default-ON.
//!
//! Upstream `code_puppy_core_plugins` **v0.0.40** names `invoke_agent` in exactly four command
//! files — `flux_bootstrap/bundled/commands/flux/exec.md:181`, `aug.md:158`, `qa.md:163`,
//! `review.md:110` — and `invoke_agent` is a CORE code-puppy tool (`code_puppy`
//! `tools/__init__.py` `TOOL_REGISTRY`), so upstream never has to ask whether it exists. cyrup's
//! rename target `subagent` (`cyrup_ext_subagents::extension::TOOL_NAME`) is registered only
//! behind the opt-in `is_installed` gate (`CYRUP_SUBAGENTS` truthy, or a `subagents/config.json`
//! at user/project scope — `extension/host/registration.rs`), whereas
//! `cyrup_flux::flux_extension_for_env` attaches at every top-level session. The port therefore
//! carries an availability pre-condition upstream never needed: each of the four templates must
//! tell the model to check its tool list for `subagent` BEFORE calling it, and to degrade to the
//! sequential single-task path (which `exec`/`aug`/`qa` already document for `1`/`all`) with one
//! user-visible notice when it is absent.
//!
//! These tests pin that contract over the SHIPPED files, read from the embedded bundle the
//! extension materialises and contributes to `ResourcesDiscover` (FLUX-001: `build.rs` embeds
//! `resources/**`, so the bytes read here are the bytes every install serves), so a template
//! regression fails here rather than mid-pipeline on a default install. They were red against the
//! pre-fix templates (no fallback sentence anywhere under `resources/prompts/flux/`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cyrup_flux::bundle::bundled_file;

/// The exact rename-map census from `spec/flux.md` §0.3 (`invoke_agent` → `subagent`,
/// "4 — aug, exec, qa, review"), matching upstream v0.0.40's four `invoke_agent` sites.
const MULTI_TASK_TEMPLATES: [&str; 4] = ["aug", "exec", "qa", "review"];

/// The one sentence every multi-task branch must open its availability check with. Spelled
/// identically in all four templates so a reader (and this test) can grep for it.
const FALLBACK_TRIGGER: &str = "If the `subagent` tool is NOT in your tool list";

/// The three `N`-argument templates whose dispatch block must route a missing tool to the
/// sequential path — `review` has no `$ARGUMENTS` dispatch block and degrades in STEP 6 instead.
const DISPATCHING_TEMPLATES: [&str; 3] = ["aug", "exec", "qa"];

fn embedded(rel: &str) -> String {
    let bytes = bundled_file(rel).unwrap_or_else(|| panic!("{rel} is not in the embedded bundle"));
    String::from_utf8(bytes.to_vec()).unwrap_or_else(|e| panic!("{rel} is not UTF-8: {e}"))
}

fn template(name: &str) -> String {
    embedded(&format!("prompts/flux/{name}.md"))
}

fn skill_md() -> String {
    embedded("skills/flux/SKILL.md")
}

#[test]
fn every_multi_task_template_still_names_the_subagent_tool_for_the_armed_path() {
    // The armed path (gate ON) is unchanged: with `CYRUP_SUBAGENTS` set the templates must still
    // issue the `subagent` call — the fix adds a pre-condition, it does not remove the fan-out.
    for name in MULTI_TASK_TEMPLATES {
        let body = template(name);
        assert!(
            body.contains("`subagent` tool"),
            "{name}.md no longer names the `subagent` tool"
        );
    }
}

#[test]
fn every_multi_task_template_checks_tool_availability_before_calling_subagent() {
    for name in MULTI_TASK_TEMPLATES {
        let body = template(name);
        let Some(trigger_at) = body.find(FALLBACK_TRIGGER) else {
            panic!("{name}.md has no `subagent` availability fallback ({FALLBACK_TRIGGER:?})");
        };
        // The fallback must be stated once, and the user must be told exactly once — the row's
        // Verify: "emit one notice explaining the degrade".
        let fallback = &body[trigger_at..];
        assert!(
            fallback.contains("tell the user ONCE"),
            "{name}.md's fallback does not tell the user once"
        );
        assert!(
            fallback.contains("do NOT call it")
                && fallback.contains("do NOT substitute another tool"),
            "{name}.md's fallback does not forbid calling or substituting the missing tool"
        );
        // The fallback names the opt-in that turns the tool on, so the notice is actionable —
        // `INSTALL_ENV_VAR` and the config file `is_installed` looks for.
        assert!(
            fallback.contains("CYRUP_SUBAGENTS=1") && fallback.contains("subagents/config.json"),
            "{name}.md's fallback does not name the subagents opt-in"
        );
    }
}

#[test]
fn the_three_n_argument_templates_route_a_missing_tool_to_the_sequential_path_in_dispatch() {
    for name in DISPATCHING_TEMPLATES {
        let body = template(name);
        // The dispatch block's `ELSE:` (pure integer > 1) branch is where the multi-task decision
        // is taken; the availability condition must live there too, not only in the section the
        // branch jumps to, so the model does not commit to fan-out before checking.
        let dispatch_end = body
            .find("## SINGLE-TASK MODE")
            .unwrap_or_else(|| panic!("{name}.md has no SINGLE-TASK MODE section"));
        let dispatch = &body[..dispatch_end];
        assert!(
            dispatch.contains("ONLY IF the `subagent` tool is in your tool list"),
            "{name}.md's dispatch block does not condition multi-task mode on tool availability"
        );
        assert!(
            dispatch.contains("Sequential mode exactly as for 'all'"),
            "{name}.md's dispatch block does not route a missing tool to the sequential path"
        );
        // And the sequential path it routes to genuinely exists in this template.
        assert!(
            dispatch.contains("(no subagents)"),
            "{name}.md's dispatch block has no no-subagent sequential path to fall back to"
        );
    }
}

#[test]
fn review_degrades_in_line_and_covers_its_second_subagent_launch_in_step_7() {
    let body = template("review");
    let step6 = body
        .find("## STEP 6")
        .unwrap_or_else(|| panic!("review.md has no STEP 6"));
    let step7 = body
        .find("## STEP 7")
        .unwrap_or_else(|| panic!("review.md has no STEP 7"));
    assert!(step6 < step7, "review.md's STEP 6 must precede STEP 7");
    let step6_body = &body[step6..step7];
    assert!(
        step6_body.contains(FALLBACK_TRIGGER),
        "review.md's STEP 6 does not check `subagent` availability before launching"
    );
    // review has no sequential `all` path to point at — its degrade is to do the group reviews
    // in-line — and STEP 7 launches sub-agents a second time, which the fallback must cover.
    assert!(
        step6_body.contains("one group at a time"),
        "review.md's fallback does not describe the in-line per-group degrade"
    );
    assert!(
        step6_body.contains("STEP 7"),
        "review.md's fallback does not extend to STEP 7's second sub-agent launch"
    );
}

#[test]
fn only_the_four_multi_task_templates_name_the_subagent_tool() {
    // Pins the rename-map census: a fifth template growing a `subagent` call would need the same
    // pre-condition and would otherwise reintroduce this defect silently.
    // The fifteen `prompts/flux/*.md` templates — one level, so `_docs/` is not counted.
    let mut naming: Vec<String> = cyrup_flux::bundle::bundled_files()
        .iter()
        .filter_map(|f| f.rel.strip_prefix("prompts/flux/"))
        .filter(|rest| !rest.contains('/') && rest.ends_with(".md"))
        .filter(|rest| embedded(&format!("prompts/flux/{rest}")).contains("`subagent` tool"))
        .filter_map(|rest| rest.strip_suffix(".md"))
        .map(str::to_string)
        .collect();
    naming.sort();
    let mut expected: Vec<String> = MULTI_TASK_TEMPLATES
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    expected.sort();
    assert_eq!(naming, expected);
}

#[test]
fn the_skill_tells_the_model_the_n_mode_needs_the_subagent_tool() {
    // The skill is what sits in the system prompt; its argument-grammar table is where the model
    // learns that `N` fans out, so it must also learn that `N` needs the opt-in tool.
    let skill = skill_md();
    let row = skill
        .lines()
        .find(|l| l.contains("| Multi-task |"))
        .unwrap_or_else(|| panic!("SKILL.md has no Multi-task row"));
    assert!(
        row.contains("`subagent` tool") && row.contains("sequentially"),
        "SKILL.md's Multi-task row does not state the tool requirement and the sequential degrade: {row}"
    );
}
