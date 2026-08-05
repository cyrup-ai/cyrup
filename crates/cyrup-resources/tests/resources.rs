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

use cyrup_core::CancelToken;
use cyrup_resources::package::lock;
use cyrup_resources::{
    DiagnosticType, DiscoveryConfig, InstallScope, InstalledPackage, InstalledPackages,
    PackageManager, PackageSource, PackageStore, ParsedGitUrl, PinRef, ResourceHandle,
    ResourceOverrides, ResourceScope, ResourceSelector, SECURITY_CAVEAT, Skill, Theme,
    ThemeWatcher, UpdateTarget, builtin_themes, discover, expand_prompt_template,
    has_unsafe_git_install_part, parse_command_args, parse_git_url, resolve_manifest,
    substitute_args, validate_name,
};

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
    let mut parts: Vec<String> = cyrup_resources::REQUIRED_COLOR_TOKENS
        .iter()
        .map(|tok| format!("\"{tok}\":{}", json_val(find(tok).unwrap_or("#000000"))))
        .collect();
    for (k, v) in colors {
        if !cyrup_resources::REQUIRED_COLOR_TOKENS.contains(k) {
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

async fn run_discover(c: &DiscoveryConfig) -> cyrup_resources::DiscoveryReport {
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

    let skill = Skill::load(
        &p,
        ResourceScope::Cli,
        cyrup_resources::ResourceOrigin::Builtin,
    )
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

#[tokio::test]
async fn a09_2_frontmatter_body_trimmed_and_crlf_normalized() {
    use cyrup_resources::{PromptTemplate, ResourceOrigin};

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

    let theme = Theme::load(
        &active,
        ResourceScope::Cli,
        cyrup_resources::ResourceOrigin::Builtin,
    )
    .unwrap();
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
        cyrup_resources::ResourceOrigin::Builtin,
    )
    .unwrap();
    let resolved = theme.resolve();
    assert_eq!(
        resolved.roles.get("background"),
        Some(&cyrup_resources::ColorSpec::Rgb {
            r: 0x11,
            g: 0x22,
            b: 0x33
        })
    );
    assert_eq!(
        resolved.roles.get("bad"),
        Some(&cyrup_resources::ColorSpec::Inherit)
    );
    assert_eq!(
        resolved.roles.get("blank"),
        Some(&cyrup_resources::ColorSpec::Inherit)
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
    assert_eq!(m.kind, cyrup_resources::package::ManifestKind::CyrupToml);
    assert_eq!(m.skills.len(), 1);
    assert_eq!(m.prompts.len(), 1);
    assert_eq!(m.themes.len(), 1);
    assert_eq!(m.extensions.len(), 1);

    // package.json `pi` key
    let t2 = tempfile::tempdir().unwrap();
    make_package_tree(t2.path(), true, true);
    let m = resolve_manifest(t2.path()).unwrap();
    assert_eq!(m.kind, cyrup_resources::package::ManifestKind::PackageJson);
    assert_eq!(m.themes.len(), 1);

    // auto-discovery (no manifest)
    let t3 = tempfile::tempdir().unwrap();
    make_package_tree(t3.path(), false, false);
    let m = resolve_manifest(t3.path()).unwrap();
    assert_eq!(
        m.kind,
        cyrup_resources::package::ManifestKind::AutoDiscovered
    );
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

    let source = PackageSource::Git { url: "file:///fake/pkg".into(), reff: PinRef::Default };
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
    assert!(report.registry.themes.contains("midnight"), "Global git package theme resolves");
    assert!(
        report.registry.ext_crate_paths.iter().any(|p| p.ends_with("deploy")),
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
    // Tag-pinned (bulk-skipped). No network: refresh re-reads HEAD from a (possibly absent) local
    // clone and degrades gracefully.
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
        report.updated.contains(&default_id),
        "bulk update moves unpinned"
    );
    assert!(!report.updated.contains(&tag_id));

    // update(One) moves the pinned package regardless.
    let one = mgr
        .update(UpdateTarget::One(tag_id.clone()), CancelToken::new())
        .await
        .unwrap();
    assert!(
        one.updated.contains(&tag_id),
        "explicit update(One) moves a pinned package"
    );
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
    assert!(matches!(err, cyrup_resources::ResourceError::Untrusted(_)));

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

    let (skill, diags) = Skill::load_with_diagnostics(
        &p,
        ResourceScope::Cli,
        cyrup_resources::ResourceOrigin::Builtin,
    )
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

    let (skill, diags) = Skill::load_with_diagnostics(
        &p,
        ResourceScope::Cli,
        cyrup_resources::ResourceOrigin::Builtin,
    )
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

    let (skill, diags) = Skill::load_with_diagnostics(
        &p,
        ResourceScope::Cli,
        cyrup_resources::ResourceOrigin::Builtin,
    )
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
    use cyrup_resources::ColorSpec;

    // Multi-level var indirection: accent -> $a -> $b -> #0a141e (theme.ts:290-306).
    let t = Theme::parse(
        &full_theme_json("t", &[("a", "$b"), ("b", "#0a141e")], &[("accent", "$a")]),
        None,
        ResourceScope::Builtin,
        cyrup_resources::ResourceOrigin::Builtin,
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
        cyrup_resources::ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(cyc.resolve().roles.get("accent"), Some(&ColorSpec::Inherit));

    // Integer 256-color index 196 → bright red via the xterm palette (theme.ts:23-28).
    let idx = Theme::parse(
        &full_theme_json("i", &[], &[("accent", "196")]),
        None,
        ResourceScope::Builtin,
        cyrup_resources::ResourceOrigin::Builtin,
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
            cyrup_resources::ResourceOrigin::Builtin,
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
        cyrup_resources::ResourceOrigin::Builtin,
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
            cyrup_resources::ResourceOrigin::Builtin,
        )
        .is_ok(),
        "schema-complete theme accepted"
    );
}

#[test]
fn builtin_themes_carry_full_token_set_and_export() {
    use cyrup_resources::ColorSpec;
    let builtins = builtin_themes();
    let dark = builtins
        .iter()
        .find(|t| t.data.name == "dark")
        .expect("dark builtin");

    // Every required token resolves (no incomplete role map) — the gap that left cyrup-tui unable
    // to render (theme.rs:276-307 stub had only 4 non-Pi tokens).
    let resolved = dark.resolve();
    for token in cyrup_resources::REQUIRED_COLOR_TOKENS {
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
        cyrup_resources::REQUIRED_COLOR_TOKENS
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
    assert_eq!(m.kind, cyrup_resources::package::ManifestKind::PackageJson);
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
        cyrup_resources::ResourceOrigin::Builtin,
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
        cyrup_resources::ResourceOrigin::Builtin,
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
            cyrup_resources::ResourceOrigin::Builtin
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
    // npm channel dropped (R-09-021).
    assert!(matches!(
        PackageSource::parse("npm:foo@1.2.3"),
        Err(cyrup_resources::ResourceError::Unsupported)
    ));

    // ParsedGitUrl::into_source round-trips (sanity for the public type).
    let parsed: ParsedGitUrl = parse_git_url("https://github.com/u/r").unwrap();
    assert!(matches!(parsed.into_source(), PackageSource::Git { .. }));
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
    cfg.configured_packages = vec![cyrup_resources::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: cyrup_resources::PackageFilter::default(),
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
    cfg.configured_packages = vec![cyrup_resources::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: cyrup_resources::PackageFilter {
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

/// A settings-declared package that is not on disk is a LOUD diagnostic, never a silent drop and
/// never a failed discovery pass.
#[tokio::test]
async fn cfg003_missing_settings_declared_package_is_an_error_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![cyrup_resources::ConfiguredPackage {
        source: "./absent-package".into(),
        scope: InstallScope::Global,
        filter: cyrup_resources::PackageFilter::default(),
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.message.contains("absent-package"))
        .expect("a missing declared package must be reported");
    assert_eq!(d.diagnostic_type, DiagnosticType::Error);
    assert_eq!(d.resource_type, cyrup_resources::ResourceKind::Package);
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
    cfg.configured_packages = vec![cyrup_resources::ConfiguredPackage {
        source: "local-pack".into(),
        scope: InstallScope::Project,
        filter: cyrup_resources::PackageFilter::default(),
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
    cfg.configured_packages = vec![cyrup_resources::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: cyrup_resources::PackageFilter {
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
    cfg.configured_packages = vec![cyrup_resources::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: cyrup_resources::PackageFilter {
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
    cfg.configured_packages = vec![cyrup_resources::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: cyrup_resources::PackageFilter {
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
    cfg.configured_packages = vec![cyrup_resources::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: cyrup_resources::PackageFilter {
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
        cyrup_resources::ConfiguredPackage {
            source: source.clone(),
            scope: InstallScope::Project,
            filter: cyrup_resources::PackageFilter {
                autoload: Some(false),
                skills: Some(vec!["-skills/alpha".to_string()]),
                ..Default::default()
            },
        },
        cyrup_resources::ConfiguredPackage {
            source: source.clone(),
            scope: InstallScope::Global,
            filter: cyrup_resources::PackageFilter::default(),
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
        cyrup_resources::ConfiguredPackage {
            source: source.clone(),
            scope: InstallScope::Project,
            filter: cyrup_resources::PackageFilter {
                autoload: Some(false),
                skills: Some(vec!["skills/alpha/**".to_string()]),
                ..Default::default()
            },
        },
        cyrup_resources::ConfiguredPackage {
            source,
            scope: InstallScope::Global,
            filter: cyrup_resources::PackageFilter::default(),
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
/// Pi's `findAutoloadDeltaBase` (package-manager.ts:1285-1299) swaps in the user entry's source and
/// scope (`resolvedSource`/`resolvedScope`, :1232-1234) before parsing, so the pair lands on ONE
/// working tree. Without it the same relative source string resolves against `<cwd>/.cyrup` for the
/// project entry and the agent dir for the global one — two different (usually non-existent)
/// directories, and the delta would silently apply to nothing.
#[tokio::test]
async fn cfg010_project_delta_resolves_against_the_global_entrys_install_location() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    // The package lives ONLY under the global base dir — the project base dir has no such tree.
    let pkg = global.join("shared-pack");
    make_package_tree(&pkg, true, false);
    write(
        &pkg.join("skills/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![
        cyrup_resources::ConfiguredPackage {
            source: "shared-pack".into(),
            scope: InstallScope::Project,
            filter: cyrup_resources::PackageFilter {
                autoload: Some(false),
                skills: Some(vec!["-skills/alpha".to_string()]),
                ..Default::default()
            },
        },
        cyrup_resources::ConfiguredPackage {
            source: "shared-pack".into(),
            scope: InstallScope::Global,
            filter: cyrup_resources::PackageFilter::default(),
        },
    ];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| !d.message.contains("shared-pack")),
        "the delta must resolve to the global entry's tree, not to a missing project path: {:?}",
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
}
