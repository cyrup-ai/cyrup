//! SKILL.md discovery, pointers, `/skill:name`, trust gate, validation, ignore-file walk,
//! symlink dedup (A-09-1, A-09-10, G5, G7).

use std::fs;

use super::fixtures::{cfg, run_discover, skill_md, write};
use crate::{DiagnosticType, DiscoveryConfig, ResourceScope, Skill, validate_name};

// ===========================================================================
// A-09-1 / A-09-10 — SKILL.md discovery, pointer, /skill:name, trust gate, standard format
// ===========================================================================

#[tokio::test]
async fn a09_1_skill_discovery_pointer_command_and_trust_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join(".cyrup/skills/foo/SKILL.md"),
        &skill_md("foo", "use when frobbing"),
    );

    // Untrusted project: the project-scoped skill is hidden (R-09-003, fail-closed).
    let mut c = cfg(root);
    c.trusted_project = false;
    let untrusted = run_discover(&c).await;
    assert!(
        !untrusted.registry.skills.contains("foo"),
        "untrusted project must hide skills"
    );

    // Trusted project: the skill surfaces with a pointer + /skill:foo command.
    c.trusted_project = true;
    let report = run_discover(&c).await;
    let skill = report
        .registry
        .skills
        .get_name("foo")
        .expect("foo skill discovered");
    assert_eq!(skill.scope, ResourceScope::Project);
    assert_eq!(skill.command(), "/skill:foo");

    let pointer = skill.pointer();
    assert_eq!(pointer.name, "foo");
    assert_eq!(pointer.description.as_deref(), Some("use when frobbing"));
    assert!(
        pointer.path.ends_with("SKILL.md"),
        "pointer points at SKILL.md for on-demand read"
    );
}

#[tokio::test]
async fn g1_project_loose_resources_load_from_cwd_root_only() {
    // Pi loads loose project resources from a single root only — `<cwd>/.cyrup/skills`
    // (skills.ts:432 `resolve(resolvedCwd, CONFIG_DIR_NAME, "skills")`) — never every ancestor
    // between cwd and the project root. A SKILL.md in an ancestor's `.cyrup/skills` must NOT be
    // discovered; only the cwd root's loose skills load.
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path();
    let nested = proj.join("sub/dir");
    fs::create_dir_all(&nested).unwrap();

    // ancestor (project_root) loose skill — must be ignored (no ancestor over-walk)
    write(
        &proj.join(".cyrup/skills/ancestor/SKILL.md"),
        &skill_md("ancestor", "in the project root"),
    );
    // cwd loose skill — must load
    write(
        &nested.join(".cyrup/skills/here/SKILL.md"),
        &skill_md("here", "in the cwd"),
    );

    let global = proj.join("global");
    fs::create_dir_all(&global).unwrap();
    let mut c = DiscoveryConfig::new(&nested, &global);
    c.project_root = Some(proj.to_path_buf());
    c.cwd = nested.clone();
    c.trusted_project = true;

    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("here"),
        "cwd-rooted loose skill must load"
    );
    assert!(
        !report.registry.skills.contains("ancestor"),
        "ancestor `.cyrup/skills` must NOT be walked (Pi loads from cwd root only)"
    );
}

#[tokio::test]
async fn g1_agents_skills_walk_ancestors_to_git_root() {
    // Pi walks `.agents/skills` up **every ancestor** from `cwd` to the git repo root (inclusive),
    // via `collectAncestorAgentsSkillDirs` (package-manager.ts:440-459) →
    // `findGitRepoRoot` (426-438) → `projectAgentsSkillDirs` (2286-2290). It STOPS at the dir that
    // contains `.git`, and never ascends above it. This behaviour-checks the exact set of
    // `.agents/skills` roots Pi's algorithm yields for `cwd = <repo>/sub/dir`:
    //   <repo>/sub/dir/.agents/skills   (cwd)        → discovered
    //   <repo>/sub/.agents/skills       (ancestor)   → discovered
    //   <repo>/.agents/skills           (git root)   → discovered  (loop breaks here)
    //   <repo-parent>/.agents/skills    (above root) → NOT discovered
    let tmp = tempfile::tempdir().unwrap();
    // an extra parent ABOVE the git root, to prove the walk stops at `.git`.
    let repo_parent = tmp.path().join("workspace");
    let repo = repo_parent.join("repo");
    let nested = repo.join("sub/dir");
    fs::create_dir_all(&nested).unwrap();
    // mark `repo` as the git repo root (Pi tests existsSync(join(dir, ".git"))).
    fs::create_dir_all(repo.join(".git")).unwrap();

    write(
        &nested.join(".agents/skills/here/SKILL.md"),
        &skill_md("here", "cwd .agents skill"),
    );
    write(
        &repo.join("sub/.agents/skills/mid/SKILL.md"),
        &skill_md("mid", "intermediate ancestor .agents skill"),
    );
    write(
        &repo.join(".agents/skills/root/SKILL.md"),
        &skill_md("root", "git-root .agents skill"),
    );
    // ABOVE the git root — must never be walked.
    write(
        &repo_parent.join(".agents/skills/outside/SKILL.md"),
        &skill_md("outside", "above the git root"),
    );

    let global = tmp.path().join("global");
    fs::create_dir_all(&global).unwrap();
    let mut c = DiscoveryConfig::new(&nested, &global);
    // Production-realistic: trust-gated, `project_root` UNSET (the session-svc builder sets
    // `trusted_project` but never `project_root`). Proves the `.agents/skills` walk runs LIVE.
    c.cwd = nested.clone();
    c.trusted_project = true;

    let report = run_discover(&c).await;
    for name in ["here", "mid", "root"] {
        assert!(
            report.registry.skills.contains(name),
            "ancestor `.agents/skills` skill `{name}` must be discovered up to the git root"
        );
        assert_eq!(
            report.registry.skills.get_name(name).unwrap().scope,
            ResourceScope::Project,
            "ancestor `.agents/skills` loads at Project scope"
        );
    }
    assert!(
        !report.registry.skills.contains("outside"),
        "`.agents/skills` above the git repo root must NOT be walked (Pi stops at `.git`)"
    );
}

#[tokio::test]
async fn g1_agents_skills_user_tier_not_double_counted() {
    // Pi filters the ancestor `.agents/skills` walk with
    // `.filter((dir) => resolve(dir) !== resolve(userAgentsSkillsDir))` (package-manager.ts:2289),
    // so `~/.agents/skills` (cyrup `global_agents_dir/skills`, already loaded at Global scope) is
    // not re-added as a Project-scope duplicate when an ancestor walk reaches it. Here the walk has
    // no git root, so it ascends to the filesystem root and passes through `global_agents_dir`'s
    // parent — the user-tier skill must surface exactly once, at Global scope.
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("a/b");
    fs::create_dir_all(&cwd).unwrap();
    // user-tier `.agents` lives directly under the temp root so the ancestor walk crosses it.
    let global_agents = tmp.path().join(".agents");
    write(
        &global_agents.join("skills/shared/SKILL.md"),
        &skill_md("shared", "user-tier agents skill"),
    );

    let global = tmp.path().join("global");
    fs::create_dir_all(&global).unwrap();
    let mut c = DiscoveryConfig::new(&cwd, &global);
    c.global_agents_dir = global_agents.clone();
    c.project_root = Some(tmp.path().to_path_buf());
    c.cwd = cwd.clone();
    c.trusted_project = true;

    let report = run_discover(&c).await;
    let all: Vec<&Skill> = report
        .registry
        .skills
        .all()
        .iter()
        .filter(|s| s.name == "shared")
        .collect();
    assert_eq!(
        all.len(),
        1,
        "user-tier `.agents/skills` must not be double-counted by the project ancestor walk"
    );
    assert_eq!(
        all[0].scope,
        ResourceScope::Global,
        "the single `shared` candidate stays at Global scope (filtered out of the project walk)"
    );
}

#[tokio::test]
async fn a09_10_agent_skills_standard_format_loads_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    // A SKILL.md authored to the Agent Skills standard (extra unmodelled keys + allowed-tools).
    let md = "---\n\
name: pdf-processing\n\
description: Use this skill when working with PDF files\n\
allowed-tools:\n  - read\n  - bash\n\
license: MIT\n\
---\n\
# PDF Processing\n\nSteps to process a PDF.\n";
    let p = tmp.path().join("skills/pdf/SKILL.md");
    write(&p, md);

    let skill = Skill::load(&p, ResourceScope::Cli, crate::ResourceOrigin::Builtin)
        .expect("standard SKILL.md loads unchanged");
    assert_eq!(skill.name, "pdf-processing");
    assert_eq!(skill.front.name.as_deref(), Some("pdf-processing"));
    assert_eq!(
        skill.front.allowed_tools,
        vec!["read".to_string(), "bash".to_string()]
    );
    assert!(
        skill.front.extra.contains_key("license"),
        "unmodelled keys round-trip via extra"
    );

    // Body is lazily read (R-09-026) and excludes the front-matter.
    let body = skill.read_body().await.unwrap();
    assert!(body.contains("Steps to process a PDF."));
    assert!(
        !body.contains("pdf-processing"),
        "front-matter excluded from body"
    );
}

// ===========================================================================
// Skill validation, parent-dir fallback, disable-model-invocation (skills.ts)
// ===========================================================================

#[test]
fn skill_parent_dir_name_fallback_and_disable_model_invocation() {
    let tmp = tempfile::tempdir().unwrap();
    // No `name` in frontmatter → falls back to parent directory basename (skills.ts:296).
    let p = tmp.path().join("my-skill/SKILL.md");
    write(
        &p,
        "---\ndescription: use when frobbing\ndisable-model-invocation: true\n---\nBody.\n",
    );

    let (skill, diags) =
        Skill::load_with_diagnostics(&p, ResourceScope::Cli, crate::ResourceOrigin::Builtin)
            .unwrap();
    let skill = skill.expect("skill loads with a parent-dir name");
    assert_eq!(skill.name, "my-skill");
    assert!(
        skill.disable_model_invocation,
        "disable-model-invocation parsed (skills.ts:316)"
    );
    assert!(diags.is_empty(), "valid skill yields no diagnostics");
}

#[test]
fn skill_missing_description_is_dropped_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("foo/SKILL.md");
    write(&p, "---\nname: foo\n---\nBody only, no description.\n");

    let (skill, diags) =
        Skill::load_with_diagnostics(&p, ResourceScope::Cli, crate::ResourceOrigin::Builtin)
            .unwrap();
    assert!(
        skill.is_none(),
        "no-description skill is dropped (skills.ts:305-307)"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("description is required")),
        "warning diagnostic emitted for missing description"
    );
}

#[test]
fn skill_invalid_name_warns_but_still_loads() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("x/SKILL.md");
    // Uppercase + underscore violate the name charset, but a valid description keeps the skill.
    write(
        &p,
        "---\nname: Bad_Name\ndescription: use when testing\n---\nBody.\n",
    );

    let (skill, diags) =
        Skill::load_with_diagnostics(&p, ResourceScope::Cli, crate::ResourceOrigin::Builtin)
            .unwrap();
    assert!(
        skill.is_some(),
        "skill kept despite invalid name (skills.ts: still load with warnings)"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("invalid characters"))
    );

    // Direct validator checks (skills.ts:92-112).
    assert!(validate_name("good-name").is_empty());
    assert!(!validate_name("-bad").is_empty(), "leading hyphen invalid");
    assert!(
        !validate_name("a--b").is_empty(),
        "consecutive hyphens invalid"
    );
}

#[tokio::test]
async fn skill_name_collision_emits_collision_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("global/skills/dup/SKILL.md"),
        &skill_md("dup", "global one"),
    );
    write(
        &root.join(".cyrup/skills/dup/SKILL.md"),
        &skill_md("dup", "project one"),
    );

    let mut c = cfg(root);
    c.trusted_project = true;
    let report = run_discover(&c).await;

    let collision = report
        .diagnostics
        .iter()
        .find(|d| d.diagnostic_type == DiagnosticType::Collision)
        .expect("a collision diagnostic is emitted (skills.ts:410-427)");
    let detail = collision
        .collision
        .as_ref()
        .expect("collision detail present");
    assert_eq!(detail.name, "dup");
    assert_ne!(detail.winner_path, detail.loser_path);
}

// ===========================================================================
// Skill walk honors .gitignore/.ignore/.fdignore (G5)
// ===========================================================================

#[tokio::test]
async fn skill_walk_honors_ignore_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let skills_root = root.join("global/skills");
    // A per-directory ignore file scopes out one subtree (skills.ts:16,47-65,189,211).
    write(&skills_root.join(".gitignore"), "secret/\n*.fdignore\n");
    write(
        &skills_root.join("secret/hidden/SKILL.md"),
        &skill_md("hidden", "should be ignored"),
    );
    write(
        &skills_root.join("visible/SKILL.md"),
        &skill_md("visible", "should load"),
    );

    let c = cfg(root);
    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("visible"),
        "non-ignored skill loads"
    );
    assert!(
        !report.registry.skills.contains("hidden"),
        "skill under a .gitignore'd directory is skipped (G5)"
    );
}

#[tokio::test]
async fn skill_walk_ignore_negation_reincludes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let skills_root = root.join("global/skills");
    // Ignore a family of sibling dirs by glob, then re-include one via `!` negation — last match
    // wins (prefixIgnorePattern preserves negation, skills.ts:32-44). Negation cannot reach inside a
    // fully-ignored parent (standard gitignore semantics), so the re-include is a sibling.
    write(&skills_root.join(".gitignore"), "draft-*/\n!draft-keep/\n");
    write(
        &skills_root.join("draft-drop/SKILL.md"),
        &skill_md("draftdrop", "ignored skill"),
    );
    write(
        &skills_root.join("draft-keep/SKILL.md"),
        &skill_md("draftkeep", "re-included skill"),
    );

    let c = cfg(root);
    let report = run_discover(&c).await;
    assert!(
        !report.registry.skills.contains("draftdrop"),
        "draft-drop ignored by glob"
    );
    assert!(
        report.registry.skills.contains("draftkeep"),
        "draft-keep re-included via negation"
    );
}

// ===========================================================================
// Symlinked duplicate skills collapse by realpath (G7)
// ===========================================================================

#[cfg(unix)]
#[tokio::test]
async fn symlinked_duplicate_skill_collapses_without_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let skills_root = root.join("global/skills");
    write(
        &skills_root.join("real/SKILL.md"),
        &skill_md("dup", "the one true skill"),
    );
    // A symlink to the real skill directory exposes the same SKILL.md under a second name.
    std::os::unix::fs::symlink(skills_root.join("real"), skills_root.join("alias")).unwrap();

    let c = cfg(root);
    let report = run_discover(&c).await;
    assert!(report.registry.skills.contains("dup"), "skill present");
    // Reaching the same file via a symlink must NOT be reported as a name collision
    // (skills.ts:403-408 canonicalizePath + realPathSet).
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.diagnostic_type != DiagnosticType::Collision),
        "symlinked duplicate of the same file is not a collision (G7)"
    );
}
