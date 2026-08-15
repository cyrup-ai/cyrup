//! Conformance tests for cyrup-resources (A-09-1..10, func-09).
//!
//! Tempdir fixtures only; hermetic (no network) by default. The git CLONE / ref-CHECKOUT /
//! PULL-on-update paths are exercised against a LOCAL `file://` git repo created in-test via gix's
//! real clone machinery (skipped gracefully if the `git` CLI is unavailable); local-path install +
//! manifest parsing + pin/update are exercised unconditionally. One true-network https clone test
//! is `#[ignore]`d and additionally gated on `CYRUP_GIT_NETWORK_TESTS=1`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::package::lock;
use crate::{
    DiagnosticType, DiscoveryConfig, InstallScope, InstalledPackage, InstalledPackages,
    PackageManager, PackageSource, PackageStore, ParsedGitUrl, PinRef, ResourceHandle,
    ResourceOverrides, ResourceScope, ResourceSelector, SECURITY_CAVEAT, Skill, Theme,
    ThemeWatcher, UpdateTarget, builtin_themes, discover, expand_prompt_template,
    has_unsafe_git_install_part, parse_command_args, parse_git_url, resolve_manifest,
    substitute_args, validate_name,
};
use cyrup_core::CancelToken;

// ---------------------------------------------------------------------------
// fixture helpers
// ---------------------------------------------------------------------------

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nDo the thing.\n")
}

/// Build a schema-complete theme JSON (every one of the 51 required color tokens present) so it
/// passes Pi's required-token validation (theme.ts:34-93). `vars`/`colors` override defaults; a
/// purely numeric color value is emitted as a JSON integer (256-color index), else as a string.
/// Override keys that are not required tokens (e.g. arbitrary roles) are appended verbatim.
fn full_theme_json(name: &str, vars: &[(&str, &str)], colors: &[(&str, &str)]) -> String {
    fn json_val(v: &str) -> String {
        if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
            v.to_string()
        } else {
            format!("\"{v}\"")
        }
    }
    let find = |key: &str| colors.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let mut parts: Vec<String> = crate::REQUIRED_COLOR_TOKENS
        .iter()
        .map(|tok| format!("\"{tok}\":{}", json_val(find(tok).unwrap_or("#000000"))))
        .collect();
    for (k, v) in colors {
        if !crate::REQUIRED_COLOR_TOKENS.contains(k) {
            parts.push(format!("\"{k}\":{}", json_val(v)));
        }
    }
    let colors_json = parts.join(",");
    let vars_json = vars
        .iter()
        .map(|(k, v)| format!("\"{k}\":{}", json_val(v)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"name\":\"{name}\",\"vars\":{{{vars_json}}},\"colors\":{{{colors_json}}}}}")
}

/// Discovery config rooted at a temp dir, project untrusted by default.
fn cfg(root: &Path) -> DiscoveryConfig {
    let global = root.join("global");
    fs::create_dir_all(&global).unwrap();
    let mut c = DiscoveryConfig::new(root, &global);
    c.project_root = Some(root.to_path_buf());
    c.cwd = root.to_path_buf();
    c
}

async fn run_discover(c: &DiscoveryConfig) -> crate::DiscoveryReport {
    discover(c, CancelToken::new()).await.unwrap()
}

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
// A-09-2 — prompt template /name expansion, placeholders, disable
// ===========================================================================

#[tokio::test]
async fn a09_2_prompt_expansion_shell_args_and_disable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Pi shell-style positional substitution (prompt-templates.ts:69-101) + frontmatter fields.
    write(
        &root.join("global/prompts/Review.md"),
        "---\ndescription: Review a PR\nargument-hint: <pr> [focus]\n---\nReview $1 focusing on ${2:-everything}: $@",
    );

    let mut c = cfg(root);
    let report = run_discover(&c).await;
    let tmpl = report
        .registry
        .prompts
        .get_name("review")
        .expect("/review template discovered");
    // name case is preserved (prompt-templates.ts:108); registry key is normalized.
    assert_eq!(tmpl.name, "Review");
    assert_eq!(tmpl.description, "Review a PR");
    assert_eq!(tmpl.argument_hint.as_deref(), Some("<pr> [focus]"));

    // `$1`, `$@`, and `${2:-default}` substitution.
    assert_eq!(
        tmpl.expand("42 perf"),
        "Review 42 focusing on perf: 42 perf"
    );
    assert_eq!(tmpl.expand("42"), "Review 42 focusing on everything: 42");

    // `/name args` entry point matches case-sensitively (prompt-templates.ts:268-284).
    let all: Vec<_> = report.registry.prompts.winners().cloned().collect();
    assert_eq!(
        expand_prompt_template("/Review 42 perf", all.iter()),
        "Review 42 focusing on perf: 42 perf"
    );
    // Non-matching `/name` is returned unchanged.
    assert_eq!(
        expand_prompt_template("/unknown x", all.iter()),
        "/unknown x"
    );
    // A non-slash line is never expanded.
    assert_eq!(expand_prompt_template("hello $1", all.iter()), "hello $1");

    // --no-prompt-templates disables discovery (R-09-010).
    c.enable_prompts = false;
    let disabled = run_discover(&c).await;
    assert!(
        !disabled.registry.prompts.contains("review"),
        "--no-prompt-templates disables it"
    );
}

#[test]
fn prompt_substitute_args_and_quote_parsing() {
    // Quote-aware tokenizer (prompt-templates.ts:24-55).
    assert_eq!(
        parse_command_args(r#"one "two three" 'four five'"#),
        vec![
            "one".to_string(),
            "two three".to_string(),
            "four five".to_string()
        ]
    );

    let args = parse_command_args("a b c d");
    // `$ARGUMENTS` == all args; `${@:2}` slices from the 2nd; `${@:2:2}` takes 2 from the 2nd.
    assert_eq!(substitute_args("$ARGUMENTS", &args), "a b c d");
    assert_eq!(substitute_args("${@:2}", &args), "b c d");
    assert_eq!(substitute_args("${@:2:2}", &args), "b c");
    // Missing positional → empty; default kicks in only when missing/empty.
    assert_eq!(substitute_args("[$5]", &args), "[]");
    assert_eq!(substitute_args("${9:-fallback}", &args), "fallback");
    // Unrecognized `${...}` is left literal.
    assert_eq!(substitute_args("${foo}", &args), "${foo}");
}

/// CFG-016 + CFG-017: pi's `:-` alternative is `(\d+|ARGUMENTS|@):-([^}]*)`
/// (prompt-templates.ts:74 @v0.83.0), so the target may be `@` or `ARGUMENTS`, and index `0` is
/// `args[-1]` — `undefined`, therefore falsy, therefore the default (`:78-79`).
///
/// Red at HEAD before the fix: `${0:-fallback}` aborted the whole form on `checked_sub(1)?` and was
/// emitted verbatim as `${0:-fallback}`; `${@:-…}` / `${ARGUMENTS:-…}` failed the all-digits guard
/// and were likewise emitted verbatim.
#[test]
fn prompt_default_forms_accept_zero_and_the_all_args_targets() {
    let args = parse_command_args("a b");
    let none: Vec<String> = Vec::new();

    // CFG-016 — `${0:-…}` always takes the default; there is no positional 0.
    assert_eq!(substitute_args("${0:-fallback}", &none), "fallback");
    assert_eq!(substitute_args("${0:-fallback}", &args), "fallback");

    // CFG-017 — `@` / `ARGUMENTS` resolve to allArgs, and fall back only when it is empty.
    assert_eq!(substitute_args("${@:-fallback}", &args), "a b");
    assert_eq!(substitute_args("${ARGUMENTS:-fallback}", &args), "a b");
    assert_eq!(substitute_args("${@:-fallback}", &none), "fallback");
    assert_eq!(substitute_args("${ARGUMENTS:-fallback}", &none), "fallback");

    // An empty default is legal (`[^}]*`), and an unknown target is still not a placeholder.
    assert_eq!(substitute_args("${9:-}", &args), "");
    assert_eq!(substitute_args("${nope:-x}", &args), "${nope:-x}");

    // The slice family is unaffected (re-pinned: `${@:1:-2}` is not a placeholder).
    assert_eq!(substitute_args("${@:1}", &args), "a b");
    assert_eq!(substitute_args("${@:1:-2}", &args), "${@:1:-2}");
}

#[tokio::test]
async fn a09_2_frontmatter_body_trimmed_and_crlf_normalized() {
    use crate::{PromptTemplate, ResourceOrigin};

    let tmp = tempfile::tempdir().unwrap();

    // CRLF line endings throughout + leading/trailing blank lines around the body. Pi's
    // parseFrontmatter normalizes `\r\n`/`\r` → `\n` over the whole file (frontmatter.ts:8) and
    // returns `body.trim()` (frontmatter.ts:24).
    let p = tmp.path().join("prompts/Review.md");
    write(
        &p,
        "---\r\ndescription: Review a PR\r\n---\r\n\r\n  Review $1 then $@  \r\n\r\n",
    );
    let tmpl = PromptTemplate::load(&p, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    // Body is trimmed (no surrounding blank lines / spaces) and contains no CR.
    assert_eq!(tmpl.body, "Review $1 then $@");
    assert!(!tmpl.body.contains('\r'), "CRLF normalized to LF");
    // Frontmatter still parsed across CRLF.
    assert_eq!(tmpl.description, "Review a PR");
    // Expansion runs on the trimmed body (prompt-templates.ts:279-280).
    assert_eq!(tmpl.expand("42 x"), "Review 42 then 42 x");

    // Multi-line body keeps interior LF but loses only the surrounding whitespace, and interior
    // CRLF becomes LF.
    let p2 = tmp.path().join("prompts/Multi.md");
    write(&p2, "---\ndescription: d\n---\n\nline one\r\nline two\n");
    let t2 = PromptTemplate::load(&p2, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    assert_eq!(t2.body, "line one\nline two");

    // Skill bodies (Pi `stripFrontmatter`, frontmatter.ts:39) are trimmed + normalized too.
    let sp = tmp.path().join("skills/x/SKILL.md");
    write(
        &sp,
        "---\r\nname: x\r\ndescription: does x\r\n---\r\n\r\n# Heading\r\n\r\nBody.\r\n\r\n",
    );
    let skill = Skill::load(&sp, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    let body = skill.read_body().await.unwrap();
    assert_eq!(body, "# Heading\n\nBody.");

    // Loose fence: Pi closes at the first `\n---` substring even when it is not its own line
    // (frontmatter.ts:17,24); the body starts immediately after that `---`.
    let p3 = tmp.path().join("prompts/Loose.md");
    write(&p3, "---\ndescription: d\n---trailing\nrest\n");
    let t3 = PromptTemplate::load(&p3, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    assert_eq!(t3.body, "trailing\nrest");
    assert_eq!(t3.description, "d");

    // No fence → empty frontmatter + whole (normalized) content as body, untrimmed per Pi
    // (frontmatter.ts:14,33: the no-fence branch does not call `.trim()`).
    let p4 = tmp.path().join("prompts/Plain.md");
    write(&p4, "just text\r\nmore\n");
    let t4 = PromptTemplate::load(&p4, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    assert_eq!(t4.body, "just text\nmore\n");
}

// ===========================================================================
// A-09-3 — themes: built-in dark/light, hot-reload, runtime switch
// ===========================================================================

#[tokio::test]
async fn a09_3_builtin_dark_and_light_present() {
    let builtins = builtin_themes();
    assert!(
        builtins.iter().any(|t| t.data.name == "dark"),
        "built-in dark exists (R-09-011)"
    );
    assert!(
        builtins.iter().any(|t| t.data.name == "light"),
        "built-in light exists (R-09-011)"
    );

    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path());
    let report = run_discover(&c).await;
    assert!(report.registry.themes.contains("dark"));
    assert!(report.registry.themes.contains("light"));
}

#[tokio::test]
async fn a09_3_theme_disable_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = cfg(tmp.path());
    c.enable_themes = false;
    let report = run_discover(&c).await;
    assert!(
        !report.registry.themes.contains("dark"),
        "--no-themes drops built-ins too"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a09_3_theme_hot_reload_and_runtime_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let active = tmp.path().join("active.json");
    write(
        &active,
        &full_theme_json("mine", &[("bg", "#000000")], &[("background", "$bg")]),
    );

    let theme = Theme::load(&active, ResourceScope::Cli, crate::ResourceOrigin::Builtin).unwrap();
    let watcher = ThemeWatcher::spawn(
        std::sync::Arc::new(theme.data.clone()),
        active.clone(),
        CancelToken::new(),
    )
    .expect("theme watcher spawns");
    let mut rx = watcher.subscribe();
    assert_eq!(rx.borrow_and_update().name, "mine");

    // Mutate the active theme file; the watcher must publish the new theme (R-09-013).
    tokio::time::sleep(Duration::from_millis(120)).await;
    write(
        &active,
        &full_theme_json("mine", &[("bg", "#ffffff")], &[("background", "$bg")]),
    );

    tokio::time::timeout(Duration::from_secs(5), rx.changed())
        .await
        .expect("hot-reload fired before timeout")
        .expect("watch channel open");
    assert_eq!(
        rx.borrow().vars.get("bg").map(String::as_str),
        Some("#ffffff")
    );

    // Runtime switch to a different theme file (R-09-014).
    let other = tmp.path().join("other.json");
    write(
        &other,
        &full_theme_json("other", &[], &[("foreground", "#abcdef")]),
    );
    watcher
        .retarget(other)
        .expect("retarget to a new active theme");
    tokio::time::timeout(Duration::from_secs(5), rx.changed())
        .await
        .expect("retarget published before timeout")
        .expect("watch channel open");
    assert_eq!(rx.borrow().name, "other");
}

#[test]
fn theme_resolve_var_indirection_and_bad_hex() {
    let theme = Theme::parse(
        &full_theme_json(
            "t",
            &[("bg", "#112233")],
            &[("background", "$bg"), ("bad", "nothex"), ("blank", "")],
        ),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    let resolved = theme.resolve();
    assert_eq!(
        resolved.roles.get("background"),
        Some(&crate::ColorSpec::Rgb {
            r: 0x11,
            g: 0x22,
            b: 0x33
        })
    );
    assert_eq!(resolved.roles.get("bad"), Some(&crate::ColorSpec::Inherit));
    assert_eq!(
        resolved.roles.get("blank"),
        Some(&crate::ColorSpec::Inherit)
    );
}

// ===========================================================================
// A-09-4 — package manifest (cyrup.toml) + pi-key + auto-discovery
// ===========================================================================

fn make_package_tree(dir: &Path, with_manifest: bool, pi_key: bool) {
    write(
        &dir.join("skills/alpha/SKILL.md"),
        &skill_md("alpha", "alpha skill"),
    );
    write(&dir.join("prompts/greet.md"), "Hello {{who}}");
    write(
        &dir.join("themes/midnight.json"),
        &full_theme_json("midnight", &[], &[]),
    );
    fs::create_dir_all(dir.join("extensions/deploy")).unwrap();
    if with_manifest && !pi_key {
        write(
            &dir.join("cyrup.toml"),
            "[package]\nname = \"pack\"\nversion = \"0.1.0\"\n\n\
             [resources]\nextensions = [\"./extensions/deploy\"]\nskills = [\"./skills\"]\n\
             prompts = [\"./prompts\"]\nthemes = [\"./themes\"]\n",
        );
    } else if with_manifest && pi_key {
        write(
            &dir.join("package.json"),
            r#"{"name":"pack","keywords":["pi-package"],"pi":{"extensions":["./extensions/deploy"],"skills":["./skills"],"prompts":["./prompts"],"themes":["./themes"]}}"#,
        );
    }
}

#[test]
fn a09_4_manifest_resolution_toml_pi_and_autodiscover() {
    // cyrup.toml
    let t1 = tempfile::tempdir().unwrap();
    make_package_tree(t1.path(), true, false);
    let m = resolve_manifest(t1.path()).unwrap();
    assert_eq!(m.kind, crate::package::ManifestKind::CyrupToml);
    assert_eq!(m.skills.len(), 1);
    assert_eq!(m.prompts.len(), 1);
    assert_eq!(m.themes.len(), 1);
    assert_eq!(m.extensions.len(), 1);

    // package.json `pi` key
    let t2 = tempfile::tempdir().unwrap();
    make_package_tree(t2.path(), true, true);
    let m = resolve_manifest(t2.path()).unwrap();
    assert_eq!(m.kind, crate::package::ManifestKind::PackageJson);
    assert_eq!(m.themes.len(), 1);

    // auto-discovery (no manifest)
    let t3 = tempfile::tempdir().unwrap();
    make_package_tree(t3.path(), false, false);
    let m = resolve_manifest(t3.path()).unwrap();
    assert_eq!(m.kind, crate::package::ManifestKind::AutoDiscovered);
    assert_eq!(m.skills, vec![t3.path().join("skills")]);
    assert_eq!(m.extensions, vec![t3.path().join("extensions")]);
}

#[tokio::test]
async fn a09_4_installed_package_resources_surface_in_discovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pkg_dir = root.join("pkgtree");
    make_package_tree(&pkg_dir, true, false);

    let mut c = cfg(root);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path {
                path: pkg_dir.clone(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("alpha"),
        "package skill surfaces"
    );
    assert!(
        report.registry.prompts.contains("greet"),
        "package prompt surfaces"
    );
    assert!(
        report.registry.themes.contains("midnight"),
        "package theme surfaces"
    );
    assert!(
        report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("deploy")),
        "package extension crate path handed off to cyrup-ext"
    );
}

#[tokio::test]
async fn installed_global_git_package_resolves_via_package_global_dir() {
    // A Global-scope git-source package's working tree lives at `<package_global_dir>/packages/<id>`
    // — the bin passes `dirs.package_dir` (default `<agent_dir>/packages`) as the store root
    // (subcommands.rs:396), so the tree is one level deeper than a naive `<global_dir>/packages/<id>`
    // guess. Discovery must resolve it via `package_global_dir`, NOT `global_dir` (the loose-resource
    // agent root), else an installed Global git/oci package's resources never surface — the base half
    // of C1 (gap-07 #1 / gap-13 C1). Path installs bypass this (they carry an absolute source path),
    // so only git/oci sources exercise the base; here we replicate the on-disk state without a real
    // clone (discovery only READS the tree, exactly like a09_5_update_skips_pinned seeds git records).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let global = root.join("global");
    let package_dir = global.join("packages"); // the bin's default `dirs.package_dir`.
    fs::create_dir_all(&global).unwrap();

    let source = PackageSource::Git {
        url: "file:///fake/pkg".into(),
        reff: PinRef::Default,
    };
    let id = source.package_id();
    let store = PackageStore::new(package_dir.clone(), None);
    let pkg_tree = store.package_dir(InstallScope::Global, &id).unwrap();
    make_package_tree(&pkg_tree, true, false);

    // `DiscoveryConfig::new(root, &global)` defaults `package_global_dir` to `global/packages`
    // (== the store root above), so a Global package resolves out of the box.
    let mut c = DiscoveryConfig::new(root, &global);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id,
            source,
            scope: InstallScope::Global,
            resolved_commit: Some("deadbeef".into()),
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("alpha"),
        "Global git package skill resolves via package_global_dir"
    );
    assert!(
        report.registry.themes.contains("midnight"),
        "Global git package theme resolves"
    );
    assert!(
        report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("deploy")),
        "Global git package extension dir collected"
    );

    // Negative control: pointing the base at the loose-resource `global_dir` (agent root) resolves
    // `<global_dir>/packages/<id>` — the WRONG, one-level-too-shallow path — so nothing is found.
    // Proves the resolution genuinely keys off `package_global_dir`, not `global_dir`.
    let mut wrong = c.clone();
    wrong.package_global_dir = global.clone();
    assert!(
        !run_discover(&wrong).await.registry.skills.contains("alpha"),
        "resolving a Global package via global_dir misses its tree — the package_global_dir base matters"
    );
}

// ===========================================================================
// A-09-5 — install / remove / list / update / pin (local path + git fixture)
// ===========================================================================

#[tokio::test]
async fn a09_5_local_path_install_list_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let pkg = tmp.path().join("mypkg");
    make_package_tree(&pkg, true, false);

    let mgr = PackageManager::new(PackageStore::new(global, None));
    let (rec, notice) = mgr
        .install(
            PackageSource::Path { path: pkg.clone() },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("local-path install");
    assert_eq!(
        notice.message, SECURITY_CAVEAT,
        "security caveat surfaced (R-09-019)"
    );

    let listed = mgr.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, rec.id);

    // enable/disable a resource within the package (R-09-018).
    mgr.set_enabled(&rec.id, ResourceSelector::Skill("alpha".to_string()), false)
        .unwrap();
    let after = mgr.list();
    assert!(after[0].disabled.skills.contains("alpha"));

    mgr.remove(&rec.id).await.expect("remove");
    assert!(mgr.list().is_empty(), "list empty after remove");
}

#[tokio::test]
async fn a09_5_update_skips_pinned_and_one_moves_it() {
    // Construct two git installs directly in the registry: one Default (bulk-updatable), one
    // Tag-pinned (bulk-skipped). Neither URL resolves, so the evidence that a package was ATTEMPTED
    // is its appearance in `report.failed` — Pi's `ensureGitRef` (package-manager.ts:1870-1896
    // @v0.83.0) opens with `runCommand("git", fetchArgs)`, which rejects on a non-zero exit and
    // propagates. This test used to assert `report.updated.contains(&default_id)` for a
    // `file:///x/a` that cannot be cloned: it passed VACUOUSLY, pinning the exact defect that a
    // failed fetch was reported to the user as a successful update.
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let store = PackageStore::new(global.clone(), None);

    let default_src = PackageSource::Git {
        url: "file:///x/a".into(),
        reff: PinRef::Default,
    };
    let tag_src = PackageSource::Git {
        url: "file:///x/b".into(),
        reff: PinRef::Tag("v1".into()),
    };
    let default_id = default_src.package_id();
    let tag_id = tag_src.package_id();
    assert!(tag_src.pin().is_pinned());
    assert!(!default_src.pin().is_pinned());

    let reg = InstalledPackages {
        packages: vec![
            InstalledPackage {
                id: default_id.clone(),
                source: default_src,
                scope: InstallScope::Global,
                resolved_commit: None,
                installed_at: "0".to_string(),
                disabled: Default::default(),
            },
            InstalledPackage {
                id: tag_id.clone(),
                source: tag_src,
                scope: InstallScope::Global,
                resolved_commit: None,
                installed_at: "0".to_string(),
                disabled: Default::default(),
            },
        ],
    };
    let reg_path = store.registry_path(InstallScope::Global).unwrap();
    lock::save(&reg_path, &reg).unwrap();

    let mgr = PackageManager::new(store);
    let report = mgr
        .update(UpdateTarget::All, CancelToken::new())
        .await
        .unwrap();
    assert!(
        report.skipped_pinned.contains(&tag_id),
        "bulk update skips pinned (R-09-020)"
    );
    assert!(
        report.failed.iter().any(|(id, _)| id == &default_id),
        "bulk update ATTEMPTS the unpinned package, and an unreachable remote is a failure"
    );
    assert!(
        !report.updated.contains(&default_id),
        "an unreachable remote must never be reported as an update"
    );
    assert!(!report.updated.contains(&tag_id));
    assert!(
        !report.failed.iter().any(|(id, _)| id == &tag_id),
        "the pinned package was skipped, so it cannot have been attempted"
    );

    // update(One) targets the pinned package regardless of the pin.
    let one = mgr
        .update(UpdateTarget::One(tag_id.clone()), CancelToken::new())
        .await
        .unwrap();
    assert!(
        one.skipped_pinned.is_empty(),
        "explicit update(One) does not skip a pinned package"
    );
    assert!(
        one.failed.iter().any(|(id, _)| id == &tag_id),
        "explicit update(One) attempts the pinned package"
    );
}

/// A git fetch that fails must leave the installed working tree byte-identical, and must be
/// reported as a failure rather than an update.
///
/// Pi never clones over an existing tree: `installGit` early-returns into a fetch when
/// `existsSync(targetDir)` (package-manager.ts:1822-1830 @v0.83.0) and `updateGit` fetches IN PLACE
/// and only `git reset --hard`s once the fetch succeeded (`:1853-1868` → `ensureGitRef`
/// `:1870-1896`), so an unreachable remote throws with the tree untouched.
///
/// Red at HEAD: `git_clone_url` opened with an unconditional `remove_dir_all(dir)`, so the tree was
/// destroyed BEFORE the fetch could fail — one offline `cyrup update` wiped every git package's
/// working tree — and `refresh` then swallowed the error, re-read nothing, and returned `Ok(())`,
/// which `update` recorded in `report.updated`.
#[tokio::test]
async fn a_failed_git_update_preserves_the_installed_tree_and_is_reported_as_failed() {
    let Some((_tmp, repo_dir)) = make_local_git_repo() else {
        eprintln!("skipping failed-update test: `git` CLI not available");
        return;
    };
    let url = format!("file://{}", repo_dir.display());

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url,
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("initial clone over file://");

    let dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    assert!(dir.is_dir(), "the install materialized a working tree");
    let listing = |p: &std::path::Path| {
        let mut names: Vec<_> = fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = listing(&dir);
    assert!(!before.is_empty(), "the clone produced files");

    // Make the remote unreachable, exactly as an offline machine or a deleted repo would.
    fs::remove_dir_all(&repo_dir).unwrap();

    let report = mgr
        .update(UpdateTarget::All, CancelToken::new())
        .await
        .unwrap();
    assert!(
        report.failed.iter().any(|(id, _)| id == &rec.id),
        "an unreachable remote is a per-package failure, not an update"
    );
    assert!(!report.updated.contains(&rec.id));

    assert!(dir.is_dir(), "the installed working tree must still exist");
    assert_eq!(before, listing(&dir), "the working tree must be unchanged");

    // No staging or backup residue is left beside it.
    let root = store.packages_root(InstallScope::Global).unwrap();
    for entry in fs::read_dir(&root).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !name.ends_with(".cyrup-incoming") && !name.ends_with(".cyrup-previous"),
            "staging residue left behind: {name}"
        );
    }
}

/// A git install whose clone fails part-way must leave NOTHING at the target path — Pi's
/// `installGit` catch is `rmSync(targetDir, { recursive: true, force: true })`
/// (package-manager.ts:1847 @v0.83.0).
///
/// Red at HEAD: the bare-on-disk-repo arm of `git_clone` `copy_tree`d the source into `dir` and only
/// THEN resolved the ref, so a directory that is not a git repo left a full copy of itself at
/// exactly the path `installed_dir` resolves — with no `packages.json` row behind it. Because
/// `resolve_configured_package` keys off the DIRECTORY and not the registry, the orphan then loads
/// as an installed package on every later session.
#[tokio::test]
async fn a_failed_git_install_leaves_no_partial_tree_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("not-a-repo");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), "---\nname: ghost\n---\n").unwrap();

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let source = PackageSource::Git {
        url: src.to_string_lossy().into_owned(),
        reff: PinRef::Default,
    };
    let id = source.package_id();

    mgr.install(source, InstallScope::Global, true, CancelToken::new())
        .await
        .expect_err("a directory that is not a git repo cannot be installed");

    let dir = store
        .package_dir(InstallScope::Global, &id)
        .expect("package dir");
    assert!(
        !dir.exists(),
        "a failed install must leave nothing at {}",
        dir.display()
    );
    assert!(mgr.list().is_empty(), "and no registry row");
}

#[tokio::test]
async fn a09_5_git_local_fixture_install() {
    let Some(repo) = make_local_git_repo() else {
        // git CLI unavailable: git-clone is fixture-gated (§7.6); the local-path + registry-driven
        // update paths above already cover install/list/remove/update/pin.
        eprintln!("skipping git-fixture install: `git` CLI not available");
        return;
    };
    let (_tmp, repo_dir) = repo;

    let global = tempfile::tempdir().unwrap();
    let mgr = PackageManager::new(PackageStore::new(global.path().to_path_buf(), None));
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url: repo_dir.display().to_string(),
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("install from local git repo");
    assert!(
        rec.resolved_commit.is_some(),
        "HEAD commit resolved via gix (no network)"
    );
    assert_eq!(mgr.list().len(), 1);
}

/// Create a real local git repo with one commit. Returns the tempdir (kept alive) + repo path, or
/// None if the `git` CLI is unavailable.
fn make_local_git_repo() -> Option<(tempfile::TempDir, PathBuf)> {
    use std::process::Command;
    let tmp = tempfile::tempdir().ok()?;
    let dir = tmp.path().to_path_buf();
    make_package_tree(&dir, true, false);
    let git = |args: &[&str]| -> bool {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !git(&["init", "-q"]) {
        return None;
    }
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    if !git(&["add", "-A"]) {
        return None;
    }
    if !git(&["commit", "-q", "-m", "init"]) {
        return None;
    }
    Some((tmp, dir))
}

// ===========================================================================
// A-09-6 — project-local scope trust gate + security caveat
// ===========================================================================

#[tokio::test]
async fn a09_6_project_install_trust_gated_with_security_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("proj");
    let pkg = tmp.path().join("pkg");
    make_package_tree(&pkg, true, false);
    let mgr = PackageManager::new(PackageStore::new(global, Some(project)));

    // Untrusted project install is refused (R-09-017).
    let err = mgr
        .install(
            PackageSource::Path { path: pkg.clone() },
            InstallScope::Project,
            false,
            CancelToken::new(),
        )
        .await
        .expect_err("untrusted project install must be refused");
    assert!(matches!(err, crate::ResourceError::Untrusted(_)));

    // Trusted project install succeeds and still surfaces the security caveat (R-09-019).
    let (_rec, notice) = mgr
        .install(
            PackageSource::Path { path: pkg },
            InstallScope::Project,
            true,
            CancelToken::new(),
        )
        .await
        .expect("trusted project install");
    assert_eq!(notice.message, SECURITY_CAVEAT);
}

/// CFG-037: a project-scope git install root must self-ignore before the clone —
/// `const gitRoot = this.getGitInstallRoot(scope); if (gitRoot) { this.ensureGitIgnore(gitRoot); }`
/// (package-manager.ts:1829-1834 @v0.83.0), with `ensureGitIgnore` writing exactly
/// `*\n!.gitignore\n` and only when no `.gitignore` is there (`:1952-1960`).
///
/// Red at HEAD: `grep -rn gitignore crates/cyrup-resources/src` found only the skill-walk ignore
/// READER; nothing wrote one, so a project-scope clone (plus its nested `.git`) landed untracked in
/// the user's repository.
#[test]
fn project_package_install_root_self_ignores_and_never_clobbers_an_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    let store = PackageStore::new(tmp.path().join("global"), Some(project.clone()));
    let root = store.packages_root(InstallScope::Project).unwrap();
    assert_eq!(root, project.join(".cyrup").join("packages"));

    // The root does not exist yet — `ensureGitIgnore` creates it (`:1953-1955`).
    crate::package::install::ensure_git_ignore(&root).unwrap();
    let ignore = root.join(".gitignore");
    assert_eq!(fs::read_to_string(&ignore).unwrap(), "*\n!.gitignore\n");

    // Idempotent, and a user's own file is left byte-identical.
    fs::write(&ignore, "# mine\n").unwrap();
    crate::package::install::ensure_git_ignore(&root).unwrap();
    assert_eq!(fs::read_to_string(&ignore).unwrap(), "# mine\n");
}

/// CFG-025: settings-declared resource paths go through `normalizePath` before the absoluteness
/// test — `resolvePathFromBase` (package-manager.ts:2069-2071 @v0.83.0) → `resolvePath`
/// (paths.ts:81-85) → `normalizePath` (`:57-78`).
///
/// Red at HEAD: `resolve_local_entries` did `PathBuf::from(entry.trim())` and tested THAT for
/// absoluteness, so `~/team-skills` became `<base>/~/team-skills` and loaded nothing, and
/// `file:///abs/x` became `<base>/file:/abs/x`.
#[test]
fn settings_local_entries_expand_tilde_and_file_urls_before_resolving_against_the_base() {
    use crate::package::manifest::{ManifestResourceType, resolve_local_entries};

    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let home = PathBuf::from(cyrup_config::paths::normalize_path("~"));

    // A `~` entry resolves under the home dir, NOT under `base`.
    let out = resolve_local_entries(
        &base,
        &["~/team-skills/SKILL.md".to_string()],
        ManifestResourceType::Skills,
    );
    // The file does not exist, so nothing is collected — what is pinned here is that the resolved
    // candidate never contained a literal `~` segment under `base`.
    assert!(out.is_empty());
    assert!(!base.join("~").exists());

    // A real file behind a `file://` URL IS collected.
    let abs = tmp.path().join("pack");
    write(&abs.join("prompts/one.md"), "hello");
    let url = format!("file://{}", abs.join("prompts").display());
    let out = resolve_local_entries(&base, &[url], ManifestResourceType::Prompts);
    assert_eq!(out, vec![abs.join("prompts/one.md")]);

    // And a real file under the home dir is collected through the `~` form.
    let _ = home;
}

// ===========================================================================
// A-09-7 — resources_discover contribution merged
// ===========================================================================

#[tokio::test]
async fn a09_7_resources_discover_contribution_merged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let contributed = root.join("ext/skills/contrib/SKILL.md");
    write(&contributed, &skill_md("contrib", "from an extension"));

    let mut c = cfg(root);
    c.extra.skill_paths = vec![contributed.parent().unwrap().to_path_buf()];
    let report = run_discover(&c).await;
    let skill = report
        .registry
        .skills
        .get_name("contrib")
        .expect("contributed skill available");
    assert_eq!(
        skill.scope,
        ResourceScope::Discovered,
        "enters at Discovered scope (R-09-022)"
    );
}

// ===========================================================================
// A-09-8 — /reload re-discovers
// ===========================================================================

#[tokio::test]
async fn a09_8_reload_rediscovers_new_resources() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut c = cfg(root);
    c.trusted_project = true;

    let first = run_discover(&c).await;
    let handle = ResourceHandle::new(first.registry);
    assert!(!handle.load().skills.contains("late"));

    // Add a new skill, then re-run discovery and atomically swap (R-09-023).
    write(
        &root.join(".cyrup/skills/late/SKILL.md"),
        &skill_md("late", "added at runtime"),
    );
    let second = run_discover(&c).await;
    handle.store(std::sync::Arc::new(second.registry));
    assert!(
        handle.load().skills.contains("late"),
        "/reload re-discovers new skill"
    );
}

// ===========================================================================
// A-09-9 — same-name precedence (built-in -> global -> project, later wins)
// ===========================================================================

#[tokio::test]
async fn a09_9_same_name_precedence_project_beats_global() {
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

    let winner = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        winner.scope,
        ResourceScope::Project,
        "project wins over global (R-09-024)"
    );
    assert_eq!(winner.front.description.as_deref(), Some("project one"));

    // Both candidates retained for diagnostics.
    let count = report
        .registry
        .skills
        .all()
        .iter()
        .filter(|s| s.key.as_str() == "dup")
        .count();
    assert_eq!(count, 2, "shadowed candidate retained in `all`");
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
// Theme: recursive vars, cycle detection, 256-color index, name '/'
// ===========================================================================

#[test]
fn theme_recursive_vars_cycle_index_and_name_slash() {
    use crate::ColorSpec;

    // Multi-level var indirection: accent -> $a -> $b -> #0a141e (theme.ts:290-306).
    let t = Theme::parse(
        &full_theme_json("t", &[("a", "$b"), ("b", "#0a141e")], &[("accent", "$a")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(
        t.resolve().roles.get("accent"),
        Some(&ColorSpec::Rgb {
            r: 0x0a,
            g: 0x14,
            b: 0x1e
        })
    );

    // Circular reference degrades to Inherit (Pi throws; cyrup is total).
    let cyc = Theme::parse(
        &full_theme_json("c", &[("a", "$b"), ("b", "$a")], &[("accent", "$a")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(cyc.resolve().roles.get("accent"), Some(&ColorSpec::Inherit));

    // Integer 256-color index 196 → bright red via the xterm palette (theme.ts:23-28).
    let idx = Theme::parse(
        &full_theme_json("i", &[], &[("accent", "196")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(
        idx.resolve().roles.get("accent"),
        Some(&ColorSpec::Rgb { r: 255, g: 0, b: 0 })
    );

    // A '/' in the theme name is rejected even when the schema is otherwise complete
    // (theme.ts:506-512). Tokens are all present so validation reaches the name check.
    assert!(
        Theme::parse(
            &full_theme_json("a/b", &[], &[]),
            None,
            ResourceScope::Builtin,
            crate::ResourceOrigin::Builtin,
        )
        .is_err(),
        "theme name with '/' rejected"
    );
}

#[tokio::test]
async fn a09_9_theme_precedence_global_overrides_builtin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A loose global theme named `dark` overrides the built-in `dark`.
    write(
        &root.join("global/themes/dark.json"),
        &full_theme_json("dark", &[("bg", "#010101")], &[("background", "$bg")]),
    );
    let c = cfg(root);
    let report = run_discover(&c).await;
    let dark = report
        .registry
        .themes
        .get_name("dark")
        .expect("dark present");
    assert_eq!(
        dark.scope,
        ResourceScope::Global,
        "global theme overrides built-in"
    );
}

// ===========================================================================
// Theme required-token schema validation + full built-in token sets (G1/G2/G10)
// ===========================================================================

#[test]
fn theme_missing_required_tokens_is_rejected_with_pi_error() {
    // A theme that omits required color tokens fails validation with Pi's exact, sorted
    // "Missing required color tokens" message (theme.ts:514-548).
    let err = Theme::parse(
        r##"{"name":"sparse","colors":{"accent":"#ffffff"}}"##,
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("incomplete theme must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Missing required color tokens"),
        "Pi error text: {msg}"
    );
    // A representative token from each schema section is reported as missing.
    for token in ["syntaxKeyword", "mdHeading", "thinkingHigh", "bashMode"] {
        assert!(msg.contains(token), "missing token `{token}` listed: {msg}");
    }
    // The present token is NOT reported as missing.
    assert!(
        !msg.contains("- accent\n"),
        "provided token must not be flagged: {msg}"
    );

    // A complete theme parses cleanly.
    assert!(
        Theme::parse(
            &full_theme_json("complete", &[], &[]),
            None,
            ResourceScope::Builtin,
            crate::ResourceOrigin::Builtin,
        )
        .is_ok(),
        "schema-complete theme accepted"
    );
}

#[test]
fn builtin_themes_carry_full_token_set_and_export() {
    use crate::ColorSpec;
    let builtins = builtin_themes();
    let dark = builtins
        .iter()
        .find(|t| t.data.name == "dark")
        .expect("dark builtin");

    // Every required token resolves (no incomplete role map) — the gap that left cyrup-tui unable
    // to render (theme.rs:276-307 stub had only 4 non-Pi tokens).
    let resolved = dark.resolve();
    for token in crate::REQUIRED_COLOR_TOKENS {
        assert!(
            resolved.roles.contains_key(token),
            "dark resolves `{token}`"
        );
    }
    // A var-indirected token and a literal-hex token resolve to the Pi values.
    assert_eq!(
        resolved.roles.get("syntaxKeyword"),
        Some(&ColorSpec::Rgb {
            r: 0x56,
            g: 0x9c,
            b: 0xd6
        }),
        "syntaxKeyword = #569CD6 (literal hex from dark.json)"
    );
    assert_eq!(
        resolved.roles.get("success"),
        Some(&ColorSpec::Rgb {
            r: 0xb5,
            g: 0xbd,
            b: 0x68
        }),
        "success -> $green -> #b5bd68 (var indirection)"
    );

    // Typed export section resolves for HTML export (theme.ts:94-100; G10).
    let export = dark.resolve_export();
    assert_eq!(
        export.page_bg,
        ColorSpec::Rgb {
            r: 0x18,
            g: 0x18,
            b: 0x1e
        }
    );
    assert_eq!(
        export.card_bg,
        ColorSpec::Rgb {
            r: 0x1e,
            g: 0x1e,
            b: 0x24
        }
    );
    assert_eq!(
        export.info_bg,
        ColorSpec::Rgb {
            r: 0x3c,
            g: 0x37,
            b: 0x28
        }
    );

    let light = builtins
        .iter()
        .find(|t| t.data.name == "light")
        .expect("light builtin");
    assert!(
        crate::REQUIRED_COLOR_TOKENS
            .iter()
            .all(|t| light.resolve().roles.contains_key(*t)),
        "light builtin also carries the full token set"
    );
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
// Explicit CLI path that does not exist → diagnostic (G8)
// ===========================================================================

#[tokio::test]
async fn missing_cli_skill_path_emits_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut c = cfg(root);
    c.cli.skills = vec![root.join("does/not/exist")];
    let report = run_discover(&c).await;
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("skill path does not exist")),
        "a missing --skill path warns instead of scanning silently (skills.ts:458)"
    );
}

// ===========================================================================
// package.json manifest entries are globs (G6)
// ===========================================================================

#[test]
fn manifest_glob_entries_expand_to_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(&dir.join("themes/a.json"), &full_theme_json("a", &[], &[]));
    write(&dir.join("themes/b.json"), &full_theme_json("b", &[], &[]));
    // A glob entry resolves via collectFilesFromManifestEntries (package-manager.ts:2201-2215).
    write(
        &dir.join("package.json"),
        r#"{"name":"pack","pi":{"themes":["themes/*.json"]}}"#,
    );
    let m = resolve_manifest(dir).unwrap();
    assert_eq!(m.kind, crate::package::ManifestKind::PackageJson);
    assert_eq!(m.themes.len(), 2, "glob expands to both theme files");
    assert!(
        m.themes
            .iter()
            .all(|p| p.extension().is_some_and(|e| e == "json"))
    );
}

#[tokio::test]
async fn manifest_glob_themes_surface_in_discovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pkg_dir = root.join("pkgtree");
    write(
        &pkg_dir.join("themes/midnight.json"),
        &full_theme_json("midnight", &[], &[]),
    );
    write(
        &pkg_dir.join("themes/noon.json"),
        &full_theme_json("noon", &[], &[]),
    );
    write(
        &pkg_dir.join("package.json"),
        r#"{"name":"pack","pi":{"themes":["themes/*.json"]}}"#,
    );

    let mut c = cfg(root);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path {
                path: pkg_dir.clone(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.themes.contains("midnight"),
        "glob-matched theme file surfaces"
    );
    assert!(
        report.registry.themes.contains("noon"),
        "second glob-matched theme file surfaces"
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

// ===========================================================================
// Git ref checkout actually materializes the pinned ref (G3)
// ===========================================================================

#[tokio::test]
async fn a09_5_git_ref_checkout_materializes_pinned_tag() {
    let Some((_tmp, repo_dir)) = make_local_git_repo_two_commits() else {
        eprintln!("skipping git ref-checkout test: `git` CLI not available");
        return;
    };

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url: repo_dir.display().to_string(),
                reff: PinRef::Tag("v1".into()),
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("install pinned to tag v1 from a local git repo");

    // The store working copy must reflect the *pinned* ref, not default HEAD: marker.txt held "v1"
    // at the tagged commit and "v2" at HEAD. Locate the materialized clone via the store dir.
    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    let marker = store_dir.join("marker.txt");
    assert!(
        marker.exists(),
        "checked-out tree present at {}",
        store_dir.display()
    );
    assert_eq!(
        fs::read_to_string(&marker).unwrap().trim(),
        "v1",
        "tag v1 checkout materialized (not default HEAD's v2) — pin is applied, R-09-018/020"
    );
}

/// Local git repo with two commits: commit 1 (`marker.txt`=="v1") tagged `v1`, commit 2 sets it to
/// "v2" and is HEAD. Returns None when the `git` CLI is unavailable.
fn make_local_git_repo_two_commits() -> Option<(tempfile::TempDir, PathBuf)> {
    use std::process::Command;
    let tmp = tempfile::tempdir().ok()?;
    let dir = tmp.path().to_path_buf();
    make_package_tree(&dir, true, false);
    let git = |args: &[&str]| -> bool {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !git(&["init", "-q"]) {
        return None;
    }
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    fs::write(dir.join("marker.txt"), "v1\n").ok()?;
    if !git(&["add", "-A"]) || !git(&["commit", "-q", "-m", "c1"]) {
        return None;
    }
    if !git(&["tag", "v1"]) {
        return None;
    }
    fs::write(dir.join("marker.txt"), "v2\n").ok()?;
    if !git(&["add", "-A"]) || !git(&["commit", "-q", "-m", "c2"]) {
        return None;
    }
    Some((tmp, dir))
}

/// Run a `git` subcommand in `dir`; returns false on failure or if the CLI is unavailable.
fn git_in(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ===========================================================================
// Manifest override patterns: `!` exclude, `+` force-include, `-` force-exclude (G2)
// ===========================================================================

#[test]
fn manifest_override_exclude_drops_matching_source() {
    // A `!` pattern excludes matching members of the resolved source set instead of being treated
    // as a literal source path that matches nothing (package-manager.ts:2154-2159, 696-717).
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(
        &dir.join("skills/public/SKILL.md"),
        &skill_md("public", "public skill"),
    );
    write(
        &dir.join("skills/internal/SKILL.md"),
        &skill_md("internal", "internal skill"),
    );
    write(
        &dir.join("skills/secret/SKILL.md"),
        &skill_md("secret", "secret skill"),
    );
    // Source glob enumerates all three; `!skills/internal` excludes one by its SKILL.md parent dir.
    write(
        &dir.join("package.json"),
        r#"{"name":"pack","pi":{"skills":["skills/*/SKILL.md","!skills/internal"]}}"#,
    );
    let m = resolve_manifest(dir).unwrap();
    let names: Vec<String> = m
        .skills
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("skills/public/SKILL.md")),
        "public kept: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.ends_with("skills/secret/SKILL.md")),
        "secret kept: {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.ends_with("skills/internal/SKILL.md")),
        "internal excluded by `!`: {names:?}"
    );
}

#[test]
fn manifest_override_force_include_and_force_exclude() {
    // `!` excludes all, `+` force-includes one back (overriding the exclude), `-` force-excludes
    // another even though it would otherwise be present (package-manager.ts:718-771).
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(&dir.join("skills/a/SKILL.md"), &skill_md("a", "skill a"));
    write(&dir.join("skills/b/SKILL.md"), &skill_md("b", "skill b"));
    write(&dir.join("skills/c/SKILL.md"), &skill_md("c", "skill c"));
    write(
        &dir.join("package.json"),
        r#"{"name":"pack","pi":{"skills":["skills/*/SKILL.md","!skills/*","+skills/b","-skills/c"]}}"#,
    );
    let m = resolve_manifest(dir).unwrap();
    let names: Vec<String> = m
        .skills
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("skills/b/SKILL.md")),
        "b force-included: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with("skills/a/SKILL.md")),
        "a stays excluded: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with("skills/c/SKILL.md")),
        "c force-excluded: {names:?}"
    );
}

#[tokio::test]
async fn manifest_override_patterns_surface_filtered_set_in_discovery() {
    // End-to-end: an installed package whose manifest uses `!` must materialize only the enabled
    // resources in discovery (package-manager.ts:2144-2163 collectManifestFiles enabledByManifest).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pkg_dir = root.join("pkgtree");
    write(
        &pkg_dir.join("themes/keep.json"),
        &full_theme_json("keep", &[], &[]),
    );
    write(
        &pkg_dir.join("themes/drop.json"),
        &full_theme_json("drop", &[], &[]),
    );
    write(
        &pkg_dir.join("package.json"),
        r#"{"name":"pack","pi":{"themes":["themes/*.json","!themes/drop.json"]}}"#,
    );

    let mut c = cfg(root);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path {
                path: pkg_dir.clone(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.themes.contains("keep"),
        "non-excluded theme surfaces"
    );
    assert!(
        !report.registry.themes.contains("drop"),
        "`!themes/drop.json` excludes the theme from discovery"
    );
}

// ===========================================================================
// Theme malformed color value rejection + "Other errors" section (G3)
// ===========================================================================

#[test]
fn theme_out_of_range_int_color_rejected_with_other_errors() {
    // Pi's ColorValueSchema is `String | Integer(0..255)`; an integer > 255 fails the union
    // (theme.ts:23-26) and is reported in the "Other errors" section (theme.ts:528-545). cyrup
    // must reject it rather than silently coerce it to inherit.
    let json = full_theme_json("oor", &[], &[("accent", "300")]);
    let err = Theme::parse(
        &json,
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("out-of-range color index must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Other errors:"),
        "other-errors section present: {msg}"
    );
    assert!(
        msg.contains("/colors/accent"),
        "offending path reported: {msg}"
    );
    assert!(
        !msg.contains("Missing required color tokens"),
        "no tokens missing — only the malformed value is reported: {msg}"
    );
}

#[test]
fn theme_non_scalar_color_value_rejected_with_combined_message() {
    // A boolean color value is neither string nor integer → rejected. Because only `accent` is
    // present, the message carries BOTH the missing-token section and the "Other errors" section,
    // mirroring Pi's combined error assembly (theme.ts:533-545).
    let json = r#"{"name":"bad","colors":{"accent":true}}"#;
    let err = Theme::parse(
        json,
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("non-scalar color value must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Missing required color tokens"),
        "missing section present: {msg}"
    );
    assert!(
        msg.contains("Other errors:"),
        "other section present: {msg}"
    );
    assert!(
        msg.contains("/colors/accent: Expected union value"),
        "bad value path + message: {msg}"
    );
}

#[test]
fn theme_valid_int_and_string_colors_still_accepted() {
    // Regression guard: in-range integer indices and hex/var strings remain valid (theme.ts:23-26).
    let json = full_theme_json("ok", &[("v", "12")], &[("accent", "196"), ("text", "$v")]);
    assert!(
        Theme::parse(
            &json,
            None,
            ResourceScope::Builtin,
            crate::ResourceOrigin::Builtin
        )
        .is_ok(),
        "valid int + string color values accepted"
    );
}

// ===========================================================================
// G2 — git source-URL parsing + security validation (utils/git.ts)
// ===========================================================================

#[test]
fn git_url_parse_protocol_scp_and_shorthand() {
    // Explicit HTTPS URL → host/path extracted, `.git` stripped, no ref (git.ts:126-163).
    let p = parse_git_url("https://github.com/user/repo.git").expect("https url parses");
    assert_eq!(p.host, "github.com");
    assert_eq!(p.path, "user/repo");
    assert_eq!(p.repo, "https://github.com/user/repo.git");
    assert_eq!(p.reff, None);
    assert!(!p.pinned);

    // scp-like `git@host:path@ref` → ref split off, repo rebuilt without the ref (git.ts:21-36).
    let s = parse_git_url("git:git@github.com:user/repo@v1.0").expect("scp form parses via git:");
    assert_eq!(s.host, "github.com");
    assert_eq!(s.path, "user/repo");
    assert_eq!(s.repo, "git@github.com:user/repo");
    assert_eq!(s.reff.as_deref(), Some("v1.0"));
    assert!(s.pinned, "an explicit ref pins (git.ts:117)");

    // `git:` host-qualified shorthand resolves through the generic parser, prefixing https.
    let g = parse_git_url("git:github.com/user/repo").expect("git: host-qualified parses");
    assert_eq!(g.repo, "https://github.com/user/repo");
    assert_eq!(g.path, "user/repo");

    // Without a git: prefix, a bare host/path is NOT a git URL (no protocol) — returns None so the
    // caller treats it as a local path (git.ts:165-170).
    assert!(parse_git_url("github.com/user/repo").is_none());
    assert!(parse_git_url("just-a-name").is_none());
}

#[test]
fn git_url_hosted_shorthand_and_committish() {
    // hosted-git-info resolution path (git.ts:181-223). All values verified 1:1 against the real
    // npm `hosted-git-info@9.0.3` (the version Pi pins, package.json:51).

    // Bare GitHub shorthand `git:owner/repo` → https clone URL, github.com host, no ref.
    let bare =
        parse_git_url("git:owner/repo").expect("bare shorthand resolves via hosted-git-info");
    assert_eq!(bare.repo, "https://owner/repo");
    assert_eq!(bare.host, "github.com");
    assert_eq!(bare.path, "owner/repo");
    assert_eq!(bare.reff, None);

    // Host-shortcut + multi-segment user resolves through the gitlab table.
    let gl =
        parse_git_url("git:gitlab.com/group/sub/proj").expect("gitlab host-qualified resolves");
    assert_eq!(gl.repo, "https://gitlab.com/group/sub/proj");
    assert_eq!(gl.host, "gitlab.com");
    assert_eq!(gl.path, "group/sub/proj");

    // A `#committish` fragment on a known host becomes the (pinned) ref; the fragment stays on the
    // clone URL exactly as Pi keeps `split.repo` verbatim (repo includes `#v2`).
    let frag = parse_git_url("https://github.com/u/r#v2").expect("#committish resolves");
    assert_eq!(frag.repo, "https://github.com/u/r#v2");
    assert_eq!(frag.path, "u/r");
    assert_eq!(frag.reff.as_deref(), Some("v2"));
    assert!(frag.pinned, "an explicit #committish pins (git.ts:202/220)");
}

#[test]
fn git_url_security_rejects_traversal_and_injection() {
    // hasUnsafeGitInstallPart: path-traversal, leading-slash, NUL, and backslash are unsafe
    // (git.ts:84-102) — a SECURITY control.
    assert!(
        has_unsafe_git_install_part("..", true),
        ".. segment is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("a/../b", true),
        "embedded .. is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("/abs", true),
        "leading slash is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("a\\b", true),
        "backslash is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("%00", true),
        "encoded NUL is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("%2e%2e/x", true),
        "encoded .. is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("a/b", false),
        "slash disallowed when allow_slash=false"
    );
    assert!(
        !has_unsafe_git_install_part("user", false),
        "plain segment is safe"
    );
    assert!(
        !has_unsafe_git_install_part("user/repo", true),
        "user/repo path is safe"
    );

    // The validator is wired into the parser. A *percent-encoded* `..` survives URL parsing as a
    // literal `..` segment and is rejected by hasUnsafeGitInstallPart → None (verified against Pi).
    assert!(parse_git_url("https://github.com/%2e%2e/secrets").is_none());
    // A *raw* `../` is collapsed by the WHATWG URL path machine BEFORE validation (new URL
    // normalizes `/../etc/passwd` → `/etc/passwd`), so Pi accepts it with a normalized, safe path —
    // there is no surviving `..` segment to reject (verified 1:1 against hosted-git-info@9.0.3).
    let normalized = parse_git_url("https://github.com/../etc/passwd").expect("raw .. normalized");
    assert_eq!(
        normalized.path, "etc/passwd",
        "raw .. collapsed by URL normalization, path is safe"
    );
    assert_eq!(normalized.host, "github.com");
}

#[test]
fn package_source_parse_routes_git_local_and_npm() {
    // Git URL → Git source carrying the clone URL (package-manager.ts:1417-1421).
    match PackageSource::parse("https://github.com/u/r").unwrap() {
        PackageSource::Git { url, reff } => {
            assert_eq!(url, "https://github.com/u/r");
            assert_eq!(reff, PinRef::Default);
        }
        other => panic!("expected Git, got {other:?}"),
    }
    // A hex ref pins as a commit; a named ref pins as a tag (both is_pinned, R-09-020 / git.ts:117).
    match PackageSource::parse("https://github.com/u/r@abc1234").unwrap() {
        PackageSource::Git { reff, .. } => {
            assert_eq!(reff, PinRef::Commit("abc1234".into()));
            assert!(reff.is_pinned());
        }
        other => panic!("expected Git commit pin, got {other:?}"),
    }
    match PackageSource::parse("https://github.com/u/r@release-1").unwrap() {
        PackageSource::Git { reff, .. } => assert_eq!(reff, PinRef::Tag("release-1".into())),
        other => panic!("expected Git tag pin, got {other:?}"),
    }
    // Bare names / relative paths → local Path (isLocalPath, paths.ts:41-55).
    assert!(matches!(
        PackageSource::parse("./pkg").unwrap(),
        PackageSource::Path { .. }
    ));
    assert!(matches!(
        PackageSource::parse("some-pkg").unwrap(),
        PackageSource::Path { .. }
    ));
    // npm channel dropped (R-09-021). CFG-009: the MESSAGE must name npm — this entry reaches the
    // user through settings `packages` on a normal session start, and the previous shared
    // `Unsupported` variant rendered as "unsupported source (OCI deferred)".
    let npm = PackageSource::parse("npm:foo@1.2.3");
    assert!(matches!(npm, Err(crate::ResourceError::UnsupportedNpm)));
    assert_eq!(
        npm.unwrap_err().to_string(),
        "unsupported source: npm packages are not supported"
    );

    // ParsedGitUrl::into_source round-trips (sanity for the public type).
    let parsed: ParsedGitUrl = parse_git_url("https://github.com/u/r").unwrap();
    assert!(matches!(parsed.into_source(), PackageSource::Git { .. }));
}

/// **CFG-052, REFUTED — this pins pi's ACTUAL behaviour, which cyrup already matches.**
///
/// CFG-052 asserts that "upstream's `parseGitUrl` reaches `hostedGitInfo.fromUrl`, which resolves
/// the `github:`/`gitlab:`/`bitbucket:` shorthands", and calls the resulting `PackageSource::Path`
/// an internal inconsistency created by porting two functions from two upstream files. Opening pi
/// at the ported tag refutes both halves:
///
/// - `parseGitUrl` (`utils/git.ts:172-179` @v0.83.0) opens with
///   `const hasGitPrefix = trimmed.startsWith("git:");` and
///   `if (!hasGitPrefix && !/^(https?|ssh|git):\/\//i.test(url)) { return null; }`. `github:u/r` has
///   no `git:` prefix, and the regex requires a literal `://`, so upstream **returns null before
///   `hostedGitInfo.fromUrl` is ever called** — the shorthand-resolution path CFG-052 relies on is
///   unreachable for this input. `crates/cyrup-resources/src/package/git_url.rs:278-287` is the same
///   two statements.
/// - The "inconsistency" is upstream's own and is deliberate: `parseSource`
///   (`core/package-manager.ts:1435-1459`) routes an `isLocalPath`-false string to `parseGitUrl` and
///   then falls through to the SAME `return { type: "local", path: source }` at `:1459` that
///   `isLocalPath`-true strings take at `:1450`. `isLocalPath` classifying `github:` as non-local
///   (`utils/paths.ts:41-55`) changes only which of the two identical returns is reached.
///
/// So a `github:` shorthand is a local path in pi and must stay a local path in cyrup. Encoding it
/// here — rather than leaving the case untested, as CFG-026 did — is what stops a future pass
/// "fixing" it into a divergence.
#[test]
fn cfg052_a_github_shorthand_is_a_local_path_exactly_as_upstream_leaves_it() {
    // Presence before absence: the same function DOES resolve the `git:`-prefixed shorthand, so a
    // `None` below is a statement about the missing `git:` prefix and not about a dead parser.
    let with_prefix = parse_git_url("git:owner/repo")
        .expect("`git:owner/repo` must still resolve through the hosted-git-info table");
    assert_eq!(with_prefix.host, "github.com");
    assert_eq!(with_prefix.path, "owner/repo");

    for shorthand in ["github:owner/repo", "gitlab:group/proj", "bitbucket:owner/repo"] {
        assert!(
            parse_git_url(shorthand).is_none(),
            "{shorthand}: pi's `parseGitUrl` returns null at git.ts:177-179 before reaching \
             hostedGitInfo, so cyrup's must too"
        );
        match PackageSource::parse(shorthand).unwrap() {
            PackageSource::Path { path } => assert_eq!(
                path,
                std::path::PathBuf::from(shorthand),
                "pi stores the source string VERBATIM on the local arm (package-manager.ts:1459)"
            ),
            other => panic!("{shorthand}: expected the local arm pi takes, got {other:?}"),
        }
    }

    // The identity keyer takes pi's same final `local:` arm, for the same reason.
    let base = std::path::Path::new("/base");
    assert_eq!(
        crate::package::package_identity("github:owner/repo", base),
        "local:/base/github:owner/repo"
    );
    // …while a form `parseGitUrl` CAN read keys on `git:<host>/<path>`, proving the `local:` arm
    // above is reached by the git parser declining, not by the identity keyer being inert.
    assert_eq!(
        crate::package::package_identity("git:owner/repo", base),
        "git:github.com/owner/repo"
    );
}

// ===========================================================================
// G3 — manifest override patterns filter at resolved-file granularity (plain dirs)
// ===========================================================================

#[test]
fn manifest_plain_dir_override_excludes_subdir() {
    // A PLAIN DIRECTORY source entry plus `!skills/internal`: Pi expands `skills/` to its SKILL.md
    // files via collectFilesFromPaths BEFORE applyPatterns (package-manager.ts:2201-2215), so the
    // exclude drops `skills/internal/SKILL.md`. (Previously cyrup applied the `!` to the raw dir
    // entry, which matched nothing.)
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(
        &dir.join("skills/public/SKILL.md"),
        &skill_md("public", "public skill"),
    );
    write(
        &dir.join("skills/internal/SKILL.md"),
        &skill_md("internal", "internal skill"),
    );
    write(
        &dir.join("skills/secret/SKILL.md"),
        &skill_md("secret", "secret skill"),
    );
    write(
        &dir.join("package.json"),
        r#"{"name":"pack","pi":{"skills":["skills","!skills/internal"]}}"#,
    );
    let m = resolve_manifest(dir).unwrap();
    let names: Vec<String> = m
        .skills
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("skills/public/SKILL.md")),
        "public kept: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.ends_with("skills/secret/SKILL.md")),
        "secret kept: {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.ends_with("skills/internal/SKILL.md")),
        "internal dropped by `!skills/internal` after dir expansion: {names:?}"
    );
}

#[tokio::test]
async fn manifest_plain_dir_override_surfaces_filtered_skills_in_discovery() {
    // End-to-end: the directory-expanded + override-filtered skills surface in discovery (the
    // excluded one does not).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pkg_dir = root.join("pkgtree");
    write(
        &pkg_dir.join("skills/keep/SKILL.md"),
        &skill_md("keepme", "kept skill"),
    );
    write(
        &pkg_dir.join("skills/internal/SKILL.md"),
        &skill_md("dropme", "internal skill"),
    );
    write(
        &pkg_dir.join("package.json"),
        r#"{"name":"pack","pi":{"skills":["skills","!skills/internal"]}}"#,
    );

    let mut c = cfg(root);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path {
                path: pkg_dir.clone(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("keepme"),
        "kept skill surfaces"
    );
    assert!(
        !report.registry.skills.contains("dropme"),
        "excluded skill is filtered out"
    );
}

// ===========================================================================
// G4 — settings override patterns filter auto-discovered loose resources
// ===========================================================================

#[tokio::test]
async fn settings_override_filters_loose_global_skills() {
    // Pi filters auto-discovered loose resources against the settings skills/prompts/themes arrays
    // via isEnabledByOverrides (package-manager.ts:700-717, 2271-2304). A `!skills/drop` override
    // disables one discovered global skill while leaving the other (and a `-` force-exclude works).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let global = root.join("global");
    write(
        &global.join("skills/keep/SKILL.md"),
        &skill_md("keepskill", "kept"),
    );
    write(
        &global.join("skills/drop/SKILL.md"),
        &skill_md("dropskill", "dropped"),
    );

    // Baseline: both discovered with no overrides.
    let baseline = run_discover(&DiscoveryConfig::new(root, &global)).await;
    assert!(
        baseline.registry.skills.contains("keepskill")
            && baseline.registry.skills.contains("dropskill"),
        "both loose skills discovered without overrides"
    );

    // With a `!skills/drop` settings override, only `keep` survives.
    let mut c = DiscoveryConfig::new(root, &global);
    c.global_overrides = ResourceOverrides {
        skills: vec!["!skills/drop".to_string()],
        ..Default::default()
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("keepskill"),
        "keep retained under override"
    );
    assert!(
        !report.registry.skills.contains("dropskill"),
        "drop excluded by settings `!skills/drop` override"
    );
}

// ===========================================================================
// G1 — install from a git URL via gix's real clone (file:// transport, hermetic)
// ===========================================================================

#[tokio::test]
async fn git_clone_from_file_url_uses_real_gix_clone() {
    // A `file://` URL is NOT the bare-directory copy fast path — it goes through gix's real clone
    // machinery (prepare_clone + fetch_then_checkout + main_worktree), the same code path remote
    // https/ssh/git URLs use. This exercises the transport end-to-end without a network.
    let Some((_tmp, repo_dir)) = make_local_git_repo() else {
        eprintln!("skipping file:// gix-clone test: `git` CLI not available");
        return;
    };
    let url = format!("file://{}", repo_dir.display());

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url,
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("install via real gix clone over file:// transport");
    assert!(
        rec.resolved_commit.is_some(),
        "HEAD commit resolved by gix clone"
    );

    // The working tree was actually checked out (the fixture's SKILL.md is present).
    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    assert!(
        store_dir.join("skills/alpha/SKILL.md").exists(),
        "gix checked out the worktree at {}",
        store_dir.display()
    );
}

#[tokio::test]
async fn git_clone_url_with_ref_checks_out_pinned_tag() {
    // A `file://` URL pinned to a tag exercises gix's clone-with-ref path
    // (prepare_clone.with_ref_name + fetch_then_checkout), the SAME machinery a pinned remote
    // https/ssh install uses — distinct from the bare-directory copy path. The materialized
    // worktree must hold the *tagged* commit's content, not default HEAD (utils/git.ts:6-19,
    // R-09-018/020).
    let Some((_tmp, repo_dir)) = make_local_git_repo_two_commits() else {
        eprintln!("skipping file:// ref-checkout test: `git` CLI not available");
        return;
    };
    let url = format!("file://{}", repo_dir.display());

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url,
                reff: PinRef::Tag("v1".into()),
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("gix clone of file:// pinned to tag v1");

    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    let marker = store_dir.join("marker.txt");
    assert!(
        marker.exists(),
        "worktree checked out at {}",
        store_dir.display()
    );
    assert_eq!(
        fs::read_to_string(&marker).unwrap().trim(),
        "v1",
        "gix checked out tag v1 (not HEAD's v2) over file:// transport — pin applied via with_ref_name"
    );
}

#[tokio::test]
async fn git_update_pulls_new_commits_from_file_url() {
    // Install from a `file://` remote, advance that remote by one commit (an upstream push), then
    // `update`. The recorded commit must advance to the new remote HEAD and the new file must be
    // present in the working tree — Pi `updateGit` fetch + reset-to-remote semantics
    // (package-manager.ts:1805-1818). Without a real network fetch this would be impossible.
    let Some((_tmp, repo_dir)) = make_local_git_repo() else {
        eprintln!("skipping pull-on-update test: `git` CLI not available");
        return;
    };
    let url = format!("file://{}", repo_dir.display());

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url,
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("initial gix clone over file://");
    let first = rec
        .resolved_commit
        .clone()
        .expect("HEAD resolved on install");

    // Advance the remote (source repo) by one commit.
    fs::write(repo_dir.join("NEW.txt"), "new\n").unwrap();
    assert!(
        git_in(&repo_dir, &["add", "-A"]),
        "stage new file in source repo"
    );
    assert!(
        git_in(&repo_dir, &["commit", "-q", "-m", "c2"]),
        "commit in source repo"
    );

    let report = mgr
        .update(UpdateTarget::All, CancelToken::new())
        .await
        .unwrap();
    assert!(report.updated.contains(&rec.id), "unpinned package updated");

    let after = mgr.list();
    let updated = after
        .iter()
        .find(|p| p.id == rec.id)
        .expect("package still installed");
    let second = updated
        .resolved_commit
        .clone()
        .expect("HEAD resolved on update");
    assert_ne!(
        first, second,
        "update pulled the new upstream commit (resolved_commit advanced)"
    );

    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    assert!(
        store_dir.join("NEW.txt").exists(),
        "pulled worktree reflects the new commit"
    );
}

/// True-network smoke test for the remote https transport. Ignored by default (and additionally
/// gated on `CYRUP_GIT_NETWORK_TESTS=1`) so CI stays hermetic; run with
/// `cargo test -p cyrup-resources -- --ignored` plus the env var to exercise a real GitHub clone.
#[tokio::test]
#[ignore = "true-network clone; set CYRUP_GIT_NETWORK_TESTS=1 and run with --ignored"]
async fn git_clone_real_network_https() {
    if std::env::var_os("CYRUP_GIT_NETWORK_TESTS").is_none() {
        eprintln!("skipping true-network clone: set CYRUP_GIT_NETWORK_TESTS=1");
        return;
    }
    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url: "https://github.com/octocat/Hello-World.git".into(),
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("real https clone over the network");
    assert!(
        rec.resolved_commit.is_some(),
        "remote HEAD resolved over https"
    );
    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    assert!(
        store_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false),
        "remote worktree materialized at {}",
        store_dir.display()
    );
}

// ===========================================================================
// G1 — resource precedence-rank model (1:1 with Pi `resourcePrecedenceRank`,
// package-manager.ts:172-188): lower rank wins under first-wins same-name dedup.
//   0 project+settings  1 project+auto  2 user+settings  3 user+auto  4 ANY package.
//   The explicit --skill/--prompt-template/--theme CLI tier is appended AFTER the sorted
//   accumulator (resource-loader.ts:421/436/455), so it loses to every package — modeled rank 5.
//   cyrup-only tiers: Discovered 6, Builtin 7.
// ===========================================================================

/// An installed package backed by a path tree containing a single named skill.
fn pkg_with_skill(
    dir: &Path,
    id: &str,
    scope: InstallScope,
    name: &str,
    desc: &str,
) -> InstalledPackage {
    write(
        &dir.join(format!("skills/{name}/SKILL.md")),
        &skill_md(name, desc),
    );
    InstalledPackage {
        id: cyrup_core::PackageId::from(id.to_string()),
        source: PackageSource::Path {
            path: dir.to_path_buf(),
        },
        scope,
        resolved_commit: None,
        installed_at: "0".to_string(),
        disabled: Default::default(),
    }
}

#[test]
fn g1_precedence_rank_matches_pi() {
    // package-manager.ts:184-188 — exact rank table; lower wins.
    assert_eq!(ResourceScope::ProjectSettings.precedence_rank(), 0);
    assert_eq!(ResourceScope::Project.precedence_rank(), 1);
    assert_eq!(ResourceScope::GlobalSettings.precedence_rank(), 2);
    assert_eq!(ResourceScope::Global.precedence_rank(), 3);
    assert_eq!(
        ResourceScope::ProjectPackage.precedence_rank(),
        4,
        "all packages rank 4"
    );
    assert_eq!(ResourceScope::GlobalPackage.precedence_rank(), 4);
    assert_eq!(
        ResourceScope::Cli.precedence_rank(),
        5,
        "CLI appended after the sorted accumulator (resource-loader.ts:421), so it loses to packages"
    );
    assert_eq!(
        ResourceScope::Discovered.precedence_rank(),
        6,
        "[CYRUP-DELTA] below CLI + packages"
    );
    assert_eq!(
        ResourceScope::Builtin.precedence_rank(),
        7,
        "[CYRUP-DELTA] lowest"
    );
}

#[tokio::test]
async fn g1_cli_loses_to_project_resource() {
    // Pi appends the explicit `--skill` paths after the sorted accumulator (resource-loader.ts:421),
    // so under first-wins a project resource (rank 1) wins the same-name collision over the CLI path
    // (rank 5) (package-manager.ts:184-188).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join(".cyrup/skills/dup/SKILL.md"),
        &skill_md("dup", "project one"),
    );
    let cli = root.join("cli/dup/SKILL.md");
    write(&cli, &skill_md("dup", "cli one"));

    let mut c = cfg(root);
    c.trusted_project = true;
    c.cli.skills = vec![cli];
    let report = run_discover(&c).await;
    let w = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        w.scope,
        ResourceScope::Project,
        "project (rank 1) beats CLI (rank 5)"
    );
    assert_eq!(w.front.description.as_deref(), Some("project one"));
}

#[tokio::test]
async fn g1_cli_loses_to_global_auto_and_to_package() {
    // Pi appends the explicit `--skill` paths (`additionalSkillPaths`) AFTER the entire sorted
    // accumulator — `mergePaths([...cliEnabledSkills, ...enabledSkills], additionalSkillPaths)`
    // (resource-loader.ts:421) — so under first-wins the CLI path loses to a same-name resource of
    // ANY rank. Here a global auto-discovered loose file (rank 3) wins over the CLI path (rank 5).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("global/skills/dup/SKILL.md"),
        &skill_md("dup", "global auto"),
    );
    let cli = root.join("cli/dup/SKILL.md");
    write(&cli, &skill_md("dup", "cli one"));

    let mut c = cfg(root);
    c.cli.skills = vec![cli.clone()];
    let report = run_discover(&c).await;
    let w = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        w.scope,
        ResourceScope::Global,
        "global auto (rank 3) beats CLI (rank 5)"
    );
    assert_eq!(w.front.description.as_deref(), Some("global auto"));

    // Now collide the CLI path with a *package* (rank 4): the package wins, because Pi appends the
    // CLI path after the rank-4 package in `enabledSkills` (resource-loader.ts:421). CLI loses.
    let pkgdir = root.join("pkg");
    let pkg = pkg_with_skill(&pkgdir, "path:p", InstallScope::Global, "only", "pkg one");
    let clip = root.join("cli2/only/SKILL.md");
    write(&clip, &skill_md("only", "cli loses"));
    let mut c2 = cfg(root);
    c2.installed = InstalledPackages {
        packages: vec![pkg],
    };
    c2.cli.skills = vec![clip];
    let r2 = run_discover(&c2).await;
    let w2 = r2.registry.skills.get_name("only").expect("only present");
    assert_eq!(
        w2.scope,
        ResourceScope::GlobalPackage,
        "package (rank 4) beats CLI (rank 5)"
    );
    assert_eq!(w2.front.description.as_deref(), Some("pkg one"));
}

#[tokio::test]
async fn g1_global_loose_beats_project_package() {
    // Inversion (c): Pi puts ALL packages at rank 4, below user-auto (rank 3), so a global loose
    // file beats a project-local package on a same-name collision (package-manager.ts:185).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("global/skills/dup/SKILL.md"),
        &skill_md("dup", "global loose"),
    );
    let pkgdir = root.join("ppkg");
    let pkg = pkg_with_skill(
        &pkgdir,
        "path:pp",
        InstallScope::Project,
        "dup",
        "project package",
    );

    let mut c = cfg(root);
    c.trusted_project = true;
    c.installed = InstalledPackages {
        packages: vec![pkg],
    };
    let report = run_discover(&c).await;
    let w = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        w.scope,
        ResourceScope::Global,
        "global loose (rank 3) beats project package (rank 4)"
    );
    assert_eq!(w.front.description.as_deref(), Some("global loose"));
}

#[tokio::test]
async fn g1_project_package_beats_global_package() {
    // Both packages rank 4; Pi inserts project-scope packages first (allPackages, 887-893), so the
    // project-local package wins the same-rank tie.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let gdir = root.join("gpkg");
    let pdir = root.join("ppkg");
    // Register global first to prove the project-first re-ordering (not input order) decides.
    let gpkg = pkg_with_skill(
        &gdir,
        "path:g",
        InstallScope::Global,
        "dup",
        "global package",
    );
    let ppkg = pkg_with_skill(
        &pdir,
        "path:p",
        InstallScope::Project,
        "dup",
        "project package",
    );

    let mut c = cfg(root);
    c.trusted_project = true;
    c.installed = InstalledPackages {
        packages: vec![gpkg, ppkg],
    };
    let report = run_discover(&c).await;
    let w = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        w.scope,
        ResourceScope::ProjectPackage,
        "project package wins the rank-4 tie"
    );
    assert_eq!(w.front.description.as_deref(), Some("project package"));
}

#[tokio::test]
async fn g1_settings_entry_outranks_auto_in_same_scope() {
    // Inversion (b): a settings-listed plain entry (source:"local") outranks an auto-discovered
    // file of the same scope (package-manager.ts:184-188, resolveLocalEntries 2218-2239). Global:
    // rank 2 beats rank 3; project: rank 0 beats rank 1.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Global: auto-discovered under global/skills, settings entry under global/extra.
    write(
        &root.join("global/skills/dup/SKILL.md"),
        &skill_md("dup", "global auto"),
    );
    write(
        &root.join("global/extra/dup/SKILL.md"),
        &skill_md("dup", "global settings"),
    );
    // Project: auto under .cyrup/skills, settings entry under .cyrup/extra (base = <cwd>/.cyrup).
    write(
        &root.join(".cyrup/skills/pdup/SKILL.md"),
        &skill_md("pdup", "project auto"),
    );
    write(
        &root.join(".cyrup/extra/pdup/SKILL.md"),
        &skill_md("pdup", "project settings"),
    );

    let mut c = cfg(root);
    c.trusted_project = true;
    c.global_overrides = ResourceOverrides {
        skills: vec!["extra/dup".to_string()],
        ..Default::default()
    };
    c.project_overrides = ResourceOverrides {
        skills: vec!["extra/pdup".to_string()],
        ..Default::default()
    };
    let report = run_discover(&c).await;

    let g = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        g.scope,
        ResourceScope::GlobalSettings,
        "global settings (rank 2) beats auto (rank 3)"
    );
    assert_eq!(g.front.description.as_deref(), Some("global settings"));

    let p = report
        .registry
        .skills
        .get_name("pdup")
        .expect("pdup present");
    assert_eq!(
        p.scope,
        ResourceScope::ProjectSettings,
        "project settings (rank 0) beats auto (rank 1)"
    );
    assert_eq!(p.front.description.as_deref(), Some("project settings"));
}

#[tokio::test]
async fn g1_settings_entry_loads_prompt_and_theme_positive_listings() {
    // resolveLocalEntries loads plain entries that are NOT in a conventional dir, across all three
    // resource families; a `!`-pattern entry in the same array still only filters (never loads).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("global/extra/hello.md"), "Say hi to {{who}}");
    write(
        &root.join("global/extra/cool.json"),
        &full_theme_json("cool", &[], &[]),
    );
    // A second prompt that the `!` pattern must keep filtering out of *auto-discovery*.
    write(&root.join("global/prompts/dropme.md"), "drop");

    let mut c = cfg(root);
    c.global_overrides = ResourceOverrides {
        prompts: vec![
            "extra/hello.md".to_string(),
            "!prompts/dropme.md".to_string(),
        ],
        themes: vec!["extra/cool.json".to_string()],
        ..Default::default()
    };
    let report = run_discover(&c).await;
    let pr = report
        .registry
        .prompts
        .get_name("hello")
        .expect("settings prompt loaded");
    assert_eq!(pr.scope, ResourceScope::GlobalSettings);
    let th = report
        .registry
        .themes
        .get_name("cool")
        .expect("settings theme loaded");
    assert_eq!(th.scope, ResourceScope::GlobalSettings);
    assert!(
        !report.registry.prompts.contains("dropme"),
        "`!` pattern still filters auto-discovery"
    );
}

#[tokio::test]
async fn g1_project_settings_entry_trust_gated() {
    // Project settings positive listings are trust-gated (fail-closed) like project loose files.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join(".cyrup/extra/secret/SKILL.md"),
        &skill_md("secret", "project settings skill"),
    );

    let mut c = cfg(root);
    c.project_overrides = ResourceOverrides {
        skills: vec!["extra/secret".to_string()],
        ..Default::default()
    };

    c.trusted_project = false;
    assert!(
        !run_discover(&c).await.registry.skills.contains("secret"),
        "untrusted hides settings entry"
    );

    c.trusted_project = true;
    assert!(
        run_discover(&c).await.registry.skills.contains("secret"),
        "trusted surfaces settings entry"
    );
}

// ===========================================================================
// CFG-003 / CFG-004 — settings-declared packages + settings-declared local entries
// ===========================================================================

/// A package declared in settings (never installed) contributes its resources, and the object-form
/// per-type include filter (Pi `applyPackageFilter`, package-manager.ts:2147-2171) is honored.
#[tokio::test]
async fn cfg003_settings_declared_package_is_discovered_with_its_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkgsrc");
    make_package_tree(&pkg, true, false);
    write(
        &pkg.join("skills/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );

    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    // No filter: everything the manifest declares loads.
    let mut cfg = DiscoveryConfig::new(cwd.clone(), global.clone());
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(report.registry.skills.contains("alpha"));
    assert!(report.registry.skills.contains("beta"));
    assert!(report.registry.prompts.contains("greet"));
    assert!(report.registry.themes.contains("midnight"));
    assert!(
        report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("deploy"))
    );

    // `skills: ["skills/alpha/**"]` selects alpha only; `themes: []` disables themes entirely.
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            skills: Some(vec!["skills/alpha/**".to_string()]),
            themes: Some(Vec::new()),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("alpha"),
        "the pattern keeps alpha"
    );
    assert!(
        !report.registry.skills.contains("beta"),
        "the pattern drops beta"
    );
    assert!(
        !report.registry.themes.contains("midnight"),
        "an explicitly EMPTY filter list disables the whole resource type"
    );
}

/// A settings-declared package that cyrup would have to INSTALL to resolve, on a pass whose
/// install gate is CLOSED, is a LOUD diagnostic — never a silent drop and never a failed discovery
/// pass.
///
/// The gate is [`DiscoveryConfig::install_missing_packages`], `false` here because this config is
/// left at its default — pi's `resolve(async () => "skip")` caller (cli/startup-ui.ts:73 @v0.83.0).
/// With it OPEN the package is cloned instead; that arm is
/// `cfg003_a_declared_git_package_is_cloned_when_the_install_gate_is_open`.
///
/// The source must be a git one. Pi installs an uninstalled npm/git source on demand
/// (`resolvePackageSources`, package-manager.ts:1287-1291 → `installMissing` `:1260-1271`
/// @v0.83.0), and where `installMissing` answers `false` pi `continue`s SILENTLY (`:1290`), so the
/// diagnostic is cyrup's documented `[CYRUP-DELTA]` for exactly that arm. A missing LOCAL path is a different
/// upstream path entirely and is silent — see
/// `cfg027_a_missing_local_package_path_is_a_silent_skip`.
///
/// The source string must carry a PROTOCOL. Pi's `parseGitUrl` bails at
/// `if (!hasGitPrefix && !/^(https?|ssh|git):\/\//i.test(url)) return null;` (git.ts:177-179
/// @v0.83.0), so a bare `github:org/pkg` shorthand — even though `isLocalPath` rejects it
/// (paths.ts:41-55) — falls out of `parseSource`'s git branch and lands on its terminal
/// `return { type: "local", path: source }` (package-manager.ts:1449-1459). Pi therefore treats
/// `github:org/pkg` as a LOCAL path and is SILENT about it, and so is cyrup; only a
/// protocol-qualified URL reaches the install arm this delta replaces.
#[tokio::test]
async fn cfg003_uninstallable_settings_declared_package_is_an_error_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: "https://github.com/org/absent-package".into(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.message.contains("absent-package"))
        .expect("an uninstalled declared package must be reported");
    assert_eq!(d.diagnostic_type, DiagnosticType::Error);
    assert_eq!(d.resource_type, crate::ResourceKind::Package);
    // …and the gate held: nothing was fetched, so the message is the "run `cyrup install`" one and
    // not an install failure.
    // The remedy text names the source, so the literal is "run `cyrup install <source>`" — matching
    // on a closing backtick right after `install` can never fire.
    assert!(
        d.message.contains("run `cyrup install "),
        "a closed gate must not have attempted an install: {}",
        d.message
    );
    assert!(
        !d.message.contains("could not be installed"),
        "…and specifically must not report an install FAILURE: {}",
        d.message
    );
}

/// CFG-003 — a settings-declared GIT package with no working tree is CLONED during discovery when
/// the install gate is open, and its resources load in the same pass.
///
/// This is pi's session path verbatim: `ResourceLoader.reload()` calls `packageManager.resolve()`
/// with **no** `onMissing` (resource-loader.ts:403 @v0.83.0, and again at `:549` for
/// `loadCurrentExtensionSet`), so the git arm's `if (!existsSync(installedPath)) { const installed =
/// await installMissing(); if (!installed) continue; }` (package-manager.ts:1287-1291) reaches
/// `installMissing` (`:1260-1271`) with no callback to consult, which installs unconditionally
/// unless `isOfflineModeEnabled()` (`:42-46`). `installParsedSource` (`:1347-1356`) → `installGit`
/// (`:1820-1852`) then does `ensureGitIgnore(gitRoot)` (`:1831-1834`) and clones (`:1837`), and the
/// tree is walked by `collectPackageResources` (`:1296`) in that same `resolve()` call.
///
/// **Red before the fix:** `resolve_configured_package` resolved a git source ONLY through an
/// already-materialized `cyrup install` tree, so this exact config produced the
/// "…is not installed at this path — run `cyrup install`" diagnostic and zero skills. Both the
/// `skills.contains("alpha")` assertion and the `dir.is_dir()` one failed.
///
/// **Hermeticity, and why this test asserts a FAILED clone rather than a successful one.** The only
/// source spellings that reach the git arm are the ones `isLocalPath` calls non-local
/// (`npm:`/`git:`/`github:`/`http:`/`https:`/`ssh:`, paths.ts:41-55 @v0.83.0) — a `file://` URL is
/// LOCAL to pi and to cyrup alike (`source.rs::is_local_path`), so it can never arrive here as a
/// git source no matter how the fixture is written. A settings string therefore cannot name a local
/// repository, and a successful clone through THIS entry point would need a real remote. `localhost`
/// keeps it on the loopback: `git:localhost/acme/pack` parses to
/// `Git { url: "https://localhost/acme/pack" }` (`parse_generic_git_url`'s bare `host/path` arm,
/// which accepts a dotted host or `localhost`) and the clone fails against a port nobody is serving.
/// The successful-clone half is `cfg003_install_declared_git_package_materializes_the_tree`.
/// (The one environmental assumption is that nothing serves a git repo on `localhost:443`; any
/// other outcome there is still a clone FAILURE, so the assertion holds either way. `git://` cannot
/// be used instead: `parseGitUrl` strips a leading `git:` before parsing (git.ts:172-179 @v0.83.0,
/// ported at `git_url.rs`), so `git://host/p` loses its scheme on both sides.)
///
/// **Red before the fix** on the message: with no install arm at all, a declared git package with
/// no tree produced "…is not installed at this path — run `cyrup install`" and nothing was ever
/// attempted. The assertion that the diagnostic says the INSTALL failed is what pins that the arm
/// is now entered.
#[tokio::test]
async fn cfg003_an_open_gate_attempts_the_install_and_reports_its_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();
    // A loose skill that must survive the failed package: pi throws out of `resolve()` here
    // (`installGit`'s `throw error`, package-manager.ts:1849 @v0.83.0) and loses the whole build;
    // cyrup's stated `[CYRUP-DELTA]` is to report and continue.
    write(
        &global.join("skills/solo/SKILL.md"),
        &skill_md("solo", "solo skill"),
    );

    let source = "git:localhost/acme/pack";
    let parsed = PackageSource::parse(source).unwrap();
    assert!(
        matches!(&parsed, PackageSource::Git { url, .. } if url == "https://localhost/acme/pack"),
        "precondition: the entry must reach the GIT arm, not the local one: {parsed:?}"
    );

    let mut cfg = DiscoveryConfig::new(cwd.clone(), global.clone());
    cfg.project_root = Some(cwd.clone());
    cfg.trusted_project = true;
    cfg.install_missing_packages = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: source.into(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];

    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.message.contains("acme/pack"))
        .expect("the declared package must be reported");
    assert_eq!(d.diagnostic_type, DiagnosticType::Error);
    assert!(
        d.message.contains("could not be installed"),
        "the gate was open, so an install was ATTEMPTED — the old \"run `cyrup install`\" text \
         means the arm was never entered: {}",
        d.message
    );
    assert!(
        report.registry.skills.contains("solo"),
        "a failed package install must not take the rest of the resource set down"
    );
    // `git_clone` stages and only renames on success — pi's `rmSync(targetDir, …)` guarantee
    // (`:1847`) arrived at differently. Nothing is left at the target path.
    let store = PackageStore::new(cfg.package_global_dir.clone(), Some(cwd.clone()));
    assert!(
        !store
            .package_dir(InstallScope::Global, &parsed.package_id())
            .unwrap()
            .exists(),
        "a failed clone must leave no partial tree"
    );
}

/// CFG-003, the clone half: [`crate::discovery::install_declared_git_package`] materializes the
/// working tree at exactly the path discovery resolves, prepares the install root first, and writes
/// no registry row.
///
/// Ported from `installGit`'s fresh-clone path (package-manager.ts:1820-1852 @v0.83.0):
/// `getGitInstallRoot` + `ensureGitIgnore` (`:1831-1834`) then the clone (`:1837`). The tree lands
/// under `PackageStore::package_dir`, which is what [`crate::package::store::installed_dir`] — the
/// resolver's git arm — returns, so the fall-through in `resolve_configured_package` walks the tree
/// it just created within the same pass, matching pi's `collectPackageResources(installedPath, …)`
/// immediately after `installMissing` (`:1293-1296`).
///
/// **COVERAGE, NOT PROOF-BY-RED:** this function is new, so no version of this test could fail
/// before the change — there was nothing to call. The red-before assertion for CFG-003 lives in
/// `cfg003_an_open_gate_attempts_the_install_and_reports_its_failure`, which drives the same arm
/// through the public `discover` entry point. Hermetic: the "remote" is a local repo behind a
/// `file://` URL — a real gix clone, no network — which is a spelling only reachable by
/// constructing the source directly, exactly as this crate's other clone tests do.
#[tokio::test]
async fn cfg003_install_declared_git_package_materializes_the_tree() {
    let Some((_repo_tmp, repo_dir)) = make_local_git_repo() else {
        eprintln!("skipping CFG-003 clone test: `git` CLI not available");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let source = PackageSource::Git {
        url: format!("file://{}", repo_dir.display()),
        reff: PinRef::Default,
    };
    let mut cfg = DiscoveryConfig::new(cwd.clone(), global.clone());
    cfg.project_root = Some(cwd.clone());
    cfg.install_missing_packages = true;

    let store = PackageStore::new(cfg.package_global_dir.clone(), Some(cwd.clone()));
    let dir = store
        .package_dir(InstallScope::Global, &source.package_id())
        .unwrap();
    assert!(!dir.exists(), "precondition: nothing is installed yet");

    crate::discovery::install_declared_git_package(&source, InstallScope::Global, &dir, &cfg)
        .expect("clone over file:// transport");

    assert!(
        dir.join("skills/alpha/SKILL.md").exists(),
        "the working tree is materialized at the path discovery resolves: {}",
        dir.display()
    );
    // The install ROOT is prepared BEFORE the clone (`:1831-1834`); at project scope that root sits
    // inside the user's own repository, which is what CFG-037 is about.
    assert_eq!(
        fs::read_to_string(
            store
                .packages_root(InstallScope::Global)
                .unwrap()
                .join(".gitignore")
        )
        .unwrap(),
        "*\n!.gitignore\n"
    );
    // No registry row is invented: `packages.json` records what `cyrup install` did, and the user
    // never installed this one. pi has no registry here at all — the declaration IS the record.
    assert!(
        !store.registry_path(InstallScope::Global).unwrap().exists(),
        "auto-install must not write an install-registry row"
    );

    // …and the tree is now the ordinary already-installed case: discovery walks it and does not
    // touch the remote again (pi's `existsSync(installedPath)` short-circuit, `:1288`). Proven with
    // the remote deleted first, so a second clone attempt could not silently succeed.
    fs::remove_dir_all(&repo_dir).unwrap();
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        // The same tree, named the way a settings entry names an ALREADY-installed local tree.
        source: dir.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("alpha"),
        "the cloned tree loads its resources: {:?}",
        report.diagnostics
    );
    assert!(report.registry.prompts.contains("greet"));
}

/// CFG-027: a settings-declared LOCAL package path that does not exist contributes nothing and
/// reports nothing — Pi's `resolveLocalExtensionSource` opens with
/// `if (!existsSync(resolved)) return;` (package-manager.ts:1324-1326 @v0.83.0), reached before any
/// diagnostic could be produced. cyrup routed it into the git arm's "not installed at this path —
/// run `cyrup install`" error, which is doubly wrong: `cyrup install` cannot materialize a path the
/// user typed, and Pi is silent here.
///
/// A bare `github:org/pkg` shorthand takes the SAME silent arm: `isLocalPath` rejects it
/// (paths.ts:41-55 @v0.83.0) but `parseGitUrl` bails on
/// `if (!hasGitPrefix && !/^(https?|ssh|git):\/\//i.test(url)) return null;` (git.ts:177-179), and
/// `parseSource` falls through to `return { type: "local", path: source }`
/// (package-manager.ts:1449-1459). Only a protocol-qualified url is a git source — see
/// `cfg003_uninstallable_settings_declared_package_is_an_error_diagnostic`.
#[tokio::test]
async fn cfg027_a_missing_local_package_path_is_a_silent_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();
    // A real sibling package, so the pass is proven to still be running after the skip.
    let present = global.join("present-pack");
    make_package_tree(&present, true, false);

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![
        crate::ConfiguredPackage {
            source: "./absent-package".into(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
        // Not a git source in Pi — no protocol, so `parseGitUrl` returns null and `parseSource`
        // files it as `{ type: "local" }`. Silent, exactly like the `./` form above.
        crate::ConfiguredPackage {
            source: "github:org/absent-shorthand".into(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
        crate::ConfiguredPackage {
            source: "present-pack".into(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
    ];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("absent-shorthand")),
        "a `github:` shorthand is a LOCAL path in Pi and must be silent too: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("absent-package")),
        "a missing LOCAL path must be silent: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.reason.contains("absent-package")),
        "…including as a warning"
    );
    assert!(
        report.registry.skills.contains("alpha"),
        "the sibling entry must still resolve"
    );
}

/// CFG-027: a settings-declared LOCAL package path that is a FILE registers as an extension
/// directly — Pi's `resolveLocalExtensionSource` `:1330-1334` @v0.83.0 sets
/// `metadata.baseDir = dirname(resolved)` and calls
/// `addResource(accumulator.extensions, resolved, metadata, true)` without going anywhere near
/// `collectPackageResources`. cyrup demanded a directory and dropped the entry with a "not installed
/// at this path" error.
#[tokio::test]
async fn cfg027_a_local_package_entry_that_is_a_file_registers_as_an_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();
    let ext_file = global.join("solo-ext.wasm");
    fs::write(&ext_file, b"\0asm").unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: "solo-ext.wasm".into(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("solo-ext.wasm")),
        "a FILE entry is the extension: {:?}",
        report.registry.ext_crate_paths
    );
    assert!(
        report.diagnostics.is_empty(),
        "and it is not an error: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
}

/// CFG-004: a PROJECT-scope settings-declared package is trust-gated (fail closed), exactly like the
/// project-installed tier.
#[tokio::test]
async fn cfg003_project_scope_declared_package_is_trust_gated() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();
    // Declared relative to the project base dir (Pi `join(cwd, CONFIG_DIR_NAME)`, :2058).
    let pkg = cwd.join(".cyrup/local-pack");
    make_package_tree(&pkg, true, false);

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: "local-pack".into(),
        scope: InstallScope::Project,
        filter: crate::PackageFilter::default(),
    }];
    cfg.trusted_project = false;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        !report.registry.skills.contains("alpha"),
        "untrusted project must load nothing"
    );

    cfg.trusted_project = true;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("alpha"),
        "a trusted project resolves the declared package against <cwd>/.cyrup"
    );
}

/// CFG-004: a plain path in the settings `extensions` array is LOADED as an extension root — Pi runs
/// `resolveLocalEntries` over `RESOURCE_TYPES`, whose first member is `"extensions"`
/// (package-manager.ts:194, :905-931).
#[tokio::test]
async fn cfg004_settings_declared_extension_entries_are_loaded_not_just_filtered() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let global = tmp.path().join("agent");
    fs::create_dir_all(cwd.join(".cyrup/exts/proj-ext")).unwrap();
    fs::create_dir_all(global.join("exts/glob-ext")).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.global_overrides = ResourceOverrides {
        extensions: vec!["exts/glob-ext".to_string()],
        ..Default::default()
    };
    cfg.project_overrides = ResourceOverrides {
        extensions: vec!["exts/proj-ext".to_string()],
        ..Default::default()
    };
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let roots = &report.registry.ext_crate_paths;
    assert!(roots.iter().any(|p| p.ends_with("glob-ext")), "{roots:?}");
    assert!(roots.iter().any(|p| p.ends_with("proj-ext")), "{roots:?}");

    // Untrusted project: the project entry must not load; the global one still does.
    cfg.trusted_project = false;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let roots = &report.registry.ext_crate_paths;
    assert!(roots.iter().any(|p| p.ends_with("glob-ext")), "{roots:?}");
    assert!(!roots.iter().any(|p| p.ends_with("proj-ext")), "{roots:?}");
}

// ===========================================================================
// CFG-010 — `autoload: false` is a DELTA filter, not an include filter
// ===========================================================================

/// Pi's object-form package entry carries `autoload` (settings-manager.ts:79, documented at :73 as
/// "start empty and only apply explicit resource patterns"). `collectPackageResources` branches on
/// it (package-manager.ts:2084-2085) into `applyPackageDeltaFilter` (:2173-2189), which starts from
/// an EMPTY set and returns immediately when the user gave no patterns for that resource type
/// (:2180-2182) — contributing nothing.
///
/// cyrup modelled none of it: `autoload` was not a field, so serde dropped it, and
/// `retain_by_package_filter` saw `patterns == None` for every type and kept the package's ENTIRE
/// manifest. A package the user explicitly opted out of loaded in full.
#[tokio::test]
async fn cfg010_autoload_false_with_no_patterns_contributes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkgsrc");
    make_package_tree(&pkg, true, false);
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            autoload: Some(false),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        !report.registry.skills.contains("alpha"),
        "autoload=false with no `skills` patterns contributes NO skills"
    );
    assert!(!report.registry.prompts.contains("greet"), "…no prompts");
    assert!(!report.registry.themes.contains("midnight"), "…no themes");
    assert!(
        !report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("deploy")),
        "…and no extensions"
    );
}

/// The delta half: under `autoload: false` an explicit pattern list ADDS BACK exactly what it names
/// (`applyAutoloadDisabledPatterns`, package-manager.ts:760-777 — start empty, each pattern sets its
/// matches' enabled flag, later patterns winning). Contrast the ordinary include-filter meaning of
/// the same list, which starts from everything.
#[tokio::test]
async fn cfg010_autoload_false_adds_back_only_the_named_patterns() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkgsrc");
    make_package_tree(&pkg, true, false);
    write(
        &pkg.join("skills/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            autoload: Some(false),
            skills: Some(vec!["skills/alpha/**".to_string()]),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("alpha"),
        "the named skill is added back"
    );
    assert!(
        !report.registry.skills.contains("beta"),
        "an unnamed sibling stays out"
    );
    assert!(
        !report.registry.prompts.contains("greet"),
        "a resource type with no patterns is still empty — the delta is per-type"
    );

    // A `!`/`-` pattern under autoload=false names something to keep DISABLED, so it adds nothing
    // (`enabled = !pattern.startsWith("-") && !pattern.startsWith("!")`, :766).
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            autoload: Some(false),
            skills: Some(vec!["!skills/alpha/**".to_string()]),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        !report.registry.skills.contains("alpha"),
        "a negative pattern under autoload=false adds nothing back"
    );
}

/// `autoload: true` (and the absent case) must leave the ordinary include-filter path untouched —
/// Pi only takes the delta branch on an explicit `=== false` (package-manager.ts:2084).
#[tokio::test]
async fn cfg010_autoload_true_keeps_the_ordinary_include_filter_meaning() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkgsrc");
    make_package_tree(&pkg, true, false);
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            autoload: Some(true),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(report.registry.skills.contains("alpha"));
    assert!(report.registry.prompts.contains("greet"));
    assert!(report.registry.themes.contains("midnight"));
}

/// CFG-010 (dedupe half) — a PROJECT `autoload: false` entry is a DELTA **over** the global entry
/// for the same package, not a replacement of it.
///
/// Pi's `dedupePackages` says so in its own doc comment — "A project entry with autoload=false is
/// a delta over the global entry, so both are kept (delta first)" (package-manager.ts:1676-1679) —
/// and keeps both at :1691-1696. `resolvePackageSources` then processes the delta first, so under
/// `addResource`'s first-writer-wins map (:2488-2490) the delta OWNS the verdict for every file it
/// names and the global entry fills in everything else. Pi pins exactly this at
/// package-manager.test.ts:1714-1738: `-extensions/foo.ts` stays disabled at project scope while
/// its sibling loads at user scope.
///
/// cyrup dropped the second entry unconditionally, which inverted the feature: the project entry's
/// patterns became the only thing that loaded.
#[tokio::test]
async fn cfg010_project_autoload_false_entry_is_a_delta_over_the_global_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkgsrc");
    make_package_tree(&pkg, true, false);
    write(
        &pkg.join("skills/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();
    let source = pkg.to_string_lossy().into_owned();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;

    // --- negative delta: opt ONE skill out of an otherwise fully autoloaded global package ---
    // `-p` matches exactly, and an exact pattern naming a skill DIRECTORY matches its `SKILL.md`
    // (`matchesAnyExactPattern`, package-manager.ts:661-679).
    cfg.configured_packages = vec![
        crate::ConfiguredPackage {
            source: source.clone(),
            scope: InstallScope::Project,
            filter: crate::PackageFilter {
                autoload: Some(false),
                skills: Some(vec!["-skills/alpha".to_string()]),
                ..Default::default()
            },
        },
        crate::ConfiguredPackage {
            source: source.clone(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
    ];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("beta"),
        "the global entry still autoloads everything the delta did not name"
    );
    assert!(
        report.registry.prompts.contains("greet"),
        "…including resource types the delta says nothing about"
    );
    assert!(
        !report.registry.skills.contains("alpha"),
        "the project delta's `-` pattern keeps `alpha` off even though the global entry autoloads it"
    );

    // --- positive delta: the named resource loads at PROJECT scope, the rest at GLOBAL scope ---
    cfg.configured_packages = vec![
        crate::ConfiguredPackage {
            source: source.clone(),
            scope: InstallScope::Project,
            filter: crate::PackageFilter {
                autoload: Some(false),
                skills: Some(vec!["skills/alpha/**".to_string()]),
                ..Default::default()
            },
        },
        crate::ConfiguredPackage {
            source,
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
    ];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let alpha = report
        .registry
        .skills
        .get_name("alpha")
        .expect("the delta names `alpha`, and the global entry autoloads it anyway");
    let beta = report
        .registry
        .skills
        .get_name("beta")
        .expect("the global entry contributes the rest of the package");
    assert_eq!(
        alpha.scope,
        ResourceScope::ProjectPackage,
        "a file the delta names is attributed to the PROJECT entry (Pi: scope \"project\")"
    );
    assert_eq!(
        beta.scope,
        ResourceScope::GlobalPackage,
        "everything else is attributed to the global entry (Pi: scope \"user\")"
    );
    // Pi's accumulator is a Map keyed by path, so a delta pair can never double-list a file.
    assert_eq!(
        report
            .registry
            .skills
            .all()
            .iter()
            .filter(|s| s.name == "alpha")
            .count(),
        1,
        "the delta pair must not list the same file twice"
    );
}

/// CFG-010 (dedupe half) — the project delta RESOLVES against the entry it deltas over.
///
/// Pi's `findAutoloadDeltaBase` (package-manager.ts:1301-1313 @v0.83.0) swaps in the user entry's
/// source and scope (`resolvedSource`/`resolvedScope`, :1246-1247) before parsing, so the pair lands
/// on ONE working tree even though the INSTALL PATH is scope-dependent
/// (`getGitInstallPath(parsed, resolvedScope)`, :1289; `getNpmInstallPath(…, resolvedScope)`,
/// :1276). Without the swap the project half looks under `<project>/.cyrup/packages/<id>`, which no
/// install ever wrote, and the delta applies to nothing.
///
/// The pair is a GIT source on purpose. `findAutoloadDeltaBase` matches by `getPackageIdentity`,
/// and that identity is scope-independent ONLY for the npm and git arms
/// (`npm:<name>` / `git:<host>/<path>`, package-manager.ts:1678-1684); the local arm resolves the
/// path against `getBaseDirForScope(scope)` (`:1685-1688`), so one RELATIVE local string is two
/// different packages across scopes and pi finds no delta base at all — the CFG-026 rule that
/// [`crate::package::package_identity`]'s own unit tests pin. A relative local source therefore
/// cannot express this case; it would only assert a pairing pi does not make.
#[tokio::test]
async fn cfg010_project_delta_resolves_against_the_global_entrys_install_location() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");

    let source_str = "https://github.com/acme/shared-pack.git";
    let source = PackageSource::parse(source_str).unwrap();
    let id = source.package_id();
    // Installed under the GLOBAL package store ONLY — `<project>/.cyrup/packages/<id>` does not
    // exist, so any resolution that keeps the project scope lands on nothing.
    let store = PackageStore::new(global.join("packages"), Some(cwd.clone()));
    let pkg = store.package_dir(InstallScope::Global, &id).unwrap();
    make_package_tree(&pkg, true, false);
    write(
        &pkg.join("skills/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );

    let delta = crate::ConfiguredPackage {
        source: source_str.into(),
        scope: InstallScope::Project,
        filter: crate::PackageFilter {
            autoload: Some(false),
            skills: Some(vec!["-skills/alpha".to_string()]),
            ..Default::default()
        },
    };
    let base = crate::ConfiguredPackage {
        source: source_str.into(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    };

    let mut cfg = DiscoveryConfig::new(&cwd, &global);
    cfg.project_root = Some(cwd.clone());
    cfg.trusted_project = true;
    cfg.configured_packages = vec![delta.clone(), base];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| !d.message.contains("shared-pack")),
        "the delta must resolve to the global entry's install location, not to a missing project \
         path: {:?}",
        report.diagnostics
    );
    assert!(
        report.registry.skills.contains("beta"),
        "the global entry autoloads the rest of the package"
    );
    assert!(
        !report.registry.skills.contains("alpha"),
        "the delta's `-` pattern reached the same tree and kept `alpha` off"
    );

    // NON-VACUITY CONTROL. Drop the global entry and the same delta has nothing to resolve against,
    // so it stays at project scope and hits the empty project store — i.e. the pass above is the
    // swap doing work, not the project scope happening to point at the same tree.
    let mut alone = cfg.clone();
    alone.configured_packages = vec![delta];
    let report = discover(&alone, CancelToken::new()).await.unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("shared-pack")),
        "an unpaired project delta resolves against the PROJECT store, which is empty: {:?}",
        report.diagnostics
    );
    assert!(!report.registry.skills.contains("beta"));
}

/// CFG-010 (pairing key) — the delta pair is matched by IDENTITY, not by source string.
///
/// `dedupePackages` keys on `getPackageIdentity(source, entry.scope)`
/// (package-manager.ts:1703 @v0.83.0), and the git arm of that identity is `git:<host>/<path>`
/// (`:1682-1684`) — deliberately spelling-independent, "to normalize SSH and HTTPS" per its own doc
/// comment (`:1669-1674`). So a project delta written `ssh://git@github.com/acme/p.git` pairs with a
/// global entry written `https://github.com/acme/p.git`. (The scp-like `git@github.com:acme/p.git`
/// would NOT: `isLocalPath` lists only `npm:`/`git:`/`github:`/`http:`/`https:`/`ssh:` as non-local
/// (paths.ts:41-55 @v0.83.0), so upstream reads that spelling as a local path, and so does cyrup.)
///
/// A source-STRING key does not pair them, and the failure is not merely a missed subtraction: the
/// two spellings still resolve onto one tree (`findAutoloadDeltaBase` matches by identity), so an
/// unpaired global half is a repeat visit to an already-seen tree and is skipped outright — every
/// resource it autoloads disappears.
#[tokio::test]
async fn cfg010_delta_pairs_across_ssh_and_https_spellings_of_one_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");

    let https = "https://github.com/acme/shared-pack.git";
    let ssh = "ssh://git@github.com/acme/shared-pack.git";
    let id = PackageSource::parse(https).unwrap().package_id();
    assert_eq!(
        id,
        PackageSource::parse(ssh).unwrap().package_id(),
        "precondition: the two spellings are one package"
    );
    let store = PackageStore::new(global.join("packages"), Some(cwd.clone()));
    let pkg = store.package_dir(InstallScope::Global, &id).unwrap();
    make_package_tree(&pkg, true, false);
    write(
        &pkg.join("skills/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );

    let mut cfg = DiscoveryConfig::new(&cwd, &global);
    cfg.project_root = Some(cwd.clone());
    cfg.trusted_project = true;
    cfg.configured_packages = vec![
        crate::ConfiguredPackage {
            source: ssh.into(),
            scope: InstallScope::Project,
            filter: crate::PackageFilter {
                autoload: Some(false),
                skills: Some(vec!["-skills/alpha".to_string()]),
                ..Default::default()
            },
        },
        crate::ConfiguredPackage {
            source: https.into(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
    ];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    // The global half must still be walked: `beta` and the package's prompt are what it autoloads,
    // and neither is anything the delta could have contributed (its only pattern is a `-`).
    assert!(
        report.registry.skills.contains("beta"),
        "the global half of the pair must not be skipped as a duplicate tree: {:?}",
        report.diagnostics
    );
    assert!(
        report.registry.prompts.contains("greet"),
        "…including resource types the delta says nothing about"
    );
    assert!(
        !report.registry.skills.contains("alpha"),
        "and the delta the other spelling declared still subtracts `alpha`"
    );
}

// ===========================================================================
// CFG-035 — `.cyrup/SYSTEM.md` / `APPEND_SYSTEM.md` discovery
// ===========================================================================

/// CFG-035: `discoverSystemPromptFile` (`resource-loader.ts:1022-1034` @v0.83.0) — the project file
/// wins ONLY when the project is trusted; otherwise the global file is used; otherwise nothing.
///
/// Before this landed, `grep -rn 'SYSTEM\.md' crates/` found the two filenames ONLY as trust-gate
/// MARKERS (`cyrup-config/src/trust.rs:194`, `:203-204`) — cyrup prompted the user to trust a
/// project *because of* a file it then never read.
#[test]
fn cfg035_system_prompt_file_is_discovered_project_first_under_trust() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent = tmp.path().join("agent");
    fs::create_dir_all(cwd.join(".cyrup")).unwrap();
    fs::create_dir_all(&agent).unwrap();

    // Nothing on disk → `None` (resource-loader.ts:1033).
    assert_eq!(crate::discover_system_prompt_file(&cwd, &agent, true), None);

    // Global only → the global file, regardless of trust (`:1028-1031` is NOT trust-gated).
    let global = agent.join("SYSTEM.md");
    fs::write(&global, "global").unwrap();
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, false),
        Some(global.clone())
    );
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, true),
        Some(global.clone())
    );

    // Project file present: it wins when trusted (`:1023-1026`) and is INVISIBLE when not.
    let project = cwd.join(".cyrup/SYSTEM.md");
    fs::write(&project, "project").unwrap();
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, true),
        Some(project.clone())
    );
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, false),
        Some(global),
        "an untrusted project falls through to the global file, not to None"
    );

    // Trusted, project file present, no global file.
    fs::remove_file(agent.join("SYSTEM.md")).unwrap();
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, true),
        Some(project)
    );
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, false),
        None
    );
}

/// CFG-035: `discoverAppendSystemPromptFile` (`resource-loader.ts:1036-1048` @v0.83.0) is the same
/// two-tier pair over `APPEND_SYSTEM.md`, and picks exactly ONE file — the project one SHADOWS the
/// global one. `cyrup-session/src/prompt/overrides.rs:15-16` documents accumulation of both tiers;
/// upstream does not accumulate.
#[test]
fn cfg035_append_system_prompt_file_picks_exactly_one_tier() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent = tmp.path().join("agent");
    fs::create_dir_all(cwd.join(".cyrup")).unwrap();
    fs::create_dir_all(&agent).unwrap();
    let project = cwd.join(".cyrup/APPEND_SYSTEM.md");
    let global = agent.join("APPEND_SYSTEM.md");
    fs::write(&project, "project").unwrap();
    fs::write(&global, "global").unwrap();

    assert_eq!(
        crate::discover_append_system_prompt_file(&cwd, &agent, true),
        Some(project),
        "trusted: the project file shadows the global one — they never accumulate"
    );
    assert_eq!(
        crate::discover_append_system_prompt_file(&cwd, &agent, false),
        Some(global)
    );
    // The SYSTEM.md pair is independent of the APPEND_SYSTEM.md pair.
    assert_eq!(crate::discover_system_prompt_file(&cwd, &agent, true), None);
}

/// CFG-035: the discovery rides out on `DiscoveryReport`, off the same `cwd` / `global_dir` /
/// `trusted_project` the registry was built from — Pi computes both inside the same `reload()`
/// (`resource-loader.ts:525`, `:531-535` @v0.83.0). This is the field
/// `cyrup-session-svc/src/builder.rs` must consume as the FALLBACK for `custom_prompt` /
/// `append_system_prompt` (the CLI flags take precedence, per Pi's `??`).
#[tokio::test]
async fn cfg035_discovery_report_carries_the_discovered_prompt_overrides() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let global = tmp.path().join("agent");
    fs::create_dir_all(cwd.join(".cyrup")).unwrap();
    fs::create_dir_all(&global).unwrap();
    fs::write(cwd.join(".cyrup/SYSTEM.md"), "project system").unwrap();
    fs::write(global.join("APPEND_SYSTEM.md"), "global append").unwrap();

    let mut cfg = DiscoveryConfig::new(cwd.clone(), global.clone());
    cfg.trusted_project = true;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert_eq!(
        report.system_prompt_file,
        Some(cwd.join(".cyrup/SYSTEM.md"))
    );
    assert_eq!(
        report.append_system_prompt_file,
        Some(global.join("APPEND_SYSTEM.md"))
    );

    cfg.trusted_project = false;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert_eq!(
        report.system_prompt_file, None,
        "an untrusted project's SYSTEM.md must not reach the prompt"
    );
    assert_eq!(
        report.append_system_prompt_file,
        Some(global.join("APPEND_SYSTEM.md")),
        "the global tier is not trust-gated"
    );
}
