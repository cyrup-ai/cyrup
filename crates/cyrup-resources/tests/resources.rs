//! Conformance tests for cyrup-resources (A-09-1..10, func-09).
//!
//! Tempdir fixtures only; no network. The git-clone path is exercised against a LOCAL git repo
//! created in-test (skipped gracefully if the `git` CLI is unavailable); local-path install +
//! manifest parsing + pin/update are exercised unconditionally.
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
    PackageManager, PackageSource, PackageStore, PinRef, ResourceHandle, ResourceScope,
    ResourceSelector, SECURITY_CAVEAT, Skill, Theme, ThemeWatcher, UpdateTarget, builtin_themes,
    discover, expand_prompt_template, parse_command_args, resolve_manifest, substitute_args,
    validate_name,
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
    write(&root.join(".cyrup/skills/foo/SKILL.md"), &skill_md("foo", "use when frobbing"));

    // Untrusted project: the project-scoped skill is hidden (R-09-003, fail-closed).
    let mut c = cfg(root);
    c.trusted_project = false;
    let untrusted = run_discover(&c).await;
    assert!(!untrusted.registry.skills.contains("foo"), "untrusted project must hide skills");

    // Trusted project: the skill surfaces with a pointer + /skill:foo command.
    c.trusted_project = true;
    let report = run_discover(&c).await;
    let skill = report.registry.skills.get_name("foo").expect("foo skill discovered");
    assert_eq!(skill.scope, ResourceScope::Project);
    assert_eq!(skill.command(), "/skill:foo");

    let pointer = skill.pointer();
    assert_eq!(pointer.name, "foo");
    assert_eq!(pointer.description.as_deref(), Some("use when frobbing"));
    assert!(pointer.path.ends_with("SKILL.md"), "pointer points at SKILL.md for on-demand read");
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

    let skill = Skill::load(&p, ResourceScope::Cli, cyrup_resources::ResourceOrigin::Builtin)
        .expect("standard SKILL.md loads unchanged");
    assert_eq!(skill.name, "pdf-processing");
    assert_eq!(skill.front.name.as_deref(), Some("pdf-processing"));
    assert_eq!(skill.front.allowed_tools, vec!["read".to_string(), "bash".to_string()]);
    assert!(skill.front.extra.contains_key("license"), "unmodelled keys round-trip via extra");

    // Body is lazily read (R-09-026) and excludes the front-matter.
    let body = skill.read_body().await.unwrap();
    assert!(body.contains("Steps to process a PDF."));
    assert!(!body.contains("pdf-processing"), "front-matter excluded from body");
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
    let tmpl = report.registry.prompts.get_name("review").expect("/review template discovered");
    // name case is preserved (prompt-templates.ts:108); registry key is normalized.
    assert_eq!(tmpl.name, "Review");
    assert_eq!(tmpl.description, "Review a PR");
    assert_eq!(tmpl.argument_hint.as_deref(), Some("<pr> [focus]"));

    // `$1`, `$@`, and `${2:-default}` substitution.
    assert_eq!(tmpl.expand("42 perf"), "Review 42 focusing on perf: 42 perf");
    assert_eq!(tmpl.expand("42"), "Review 42 focusing on everything: 42");

    // `/name args` entry point matches case-sensitively (prompt-templates.ts:268-284).
    let all: Vec<_> = report.registry.prompts.winners().cloned().collect();
    assert_eq!(
        expand_prompt_template("/Review 42 perf", all.iter()),
        "Review 42 focusing on perf: 42 perf"
    );
    // Non-matching `/name` is returned unchanged.
    assert_eq!(expand_prompt_template("/unknown x", all.iter()), "/unknown x");
    // A non-slash line is never expanded.
    assert_eq!(expand_prompt_template("hello $1", all.iter()), "hello $1");

    // --no-prompt-templates disables discovery (R-09-010).
    c.enable_prompts = false;
    let disabled = run_discover(&c).await;
    assert!(!disabled.registry.prompts.contains("review"), "--no-prompt-templates disables it");
}

#[test]
fn prompt_substitute_args_and_quote_parsing() {
    // Quote-aware tokenizer (prompt-templates.ts:24-55).
    assert_eq!(
        parse_command_args(r#"one "two three" 'four five'"#),
        vec!["one".to_string(), "two three".to_string(), "four five".to_string()]
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
    assert!(builtins.iter().any(|t| t.data.name == "dark"), "built-in dark exists (R-09-011)");
    assert!(builtins.iter().any(|t| t.data.name == "light"), "built-in light exists (R-09-011)");

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
    assert!(!report.registry.themes.contains("dark"), "--no-themes drops built-ins too");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a09_3_theme_hot_reload_and_runtime_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let active = tmp.path().join("active.json");
    write(&active, &full_theme_json("mine", &[("bg", "#000000")], &[("background", "$bg")]));

    let theme = Theme::load(&active, ResourceScope::Cli, cyrup_resources::ResourceOrigin::Builtin)
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
    write(&active, &full_theme_json("mine", &[("bg", "#ffffff")], &[("background", "$bg")]));

    tokio::time::timeout(Duration::from_secs(5), rx.changed())
        .await
        .expect("hot-reload fired before timeout")
        .expect("watch channel open");
    assert_eq!(rx.borrow().vars.get("bg").map(String::as_str), Some("#ffffff"));

    // Runtime switch to a different theme file (R-09-014).
    let other = tmp.path().join("other.json");
    write(&other, &full_theme_json("other", &[], &[("foreground", "#abcdef")]));
    watcher.retarget(other).expect("retarget to a new active theme");
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
        Some(&cyrup_resources::ColorSpec::Rgb { r: 0x11, g: 0x22, b: 0x33 })
    );
    assert_eq!(resolved.roles.get("bad"), Some(&cyrup_resources::ColorSpec::Inherit));
    assert_eq!(resolved.roles.get("blank"), Some(&cyrup_resources::ColorSpec::Inherit));
}

// ===========================================================================
// A-09-4 — package manifest (cyrup.toml) + pi-key + auto-discovery
// ===========================================================================

fn make_package_tree(dir: &Path, with_manifest: bool, pi_key: bool) {
    write(&dir.join("skills/alpha/SKILL.md"), &skill_md("alpha", "alpha skill"));
    write(&dir.join("prompts/greet.md"), "Hello {{who}}");
    write(&dir.join("themes/midnight.json"), &full_theme_json("midnight", &[], &[]));
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
    assert_eq!(m.kind, cyrup_resources::package::ManifestKind::AutoDiscovered);
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
            source: PackageSource::Path { path: pkg_dir.clone() },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(report.registry.skills.contains("alpha"), "package skill surfaces");
    assert!(report.registry.prompts.contains("greet"), "package prompt surfaces");
    assert!(report.registry.themes.contains("midnight"), "package theme surfaces");
    assert!(
        report.registry.ext_crate_paths.iter().any(|p| p.ends_with("deploy")),
        "package extension crate path handed off to cyrup-ext"
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
        .install(PackageSource::Path { path: pkg.clone() }, InstallScope::Global, true, CancelToken::new())
        .await
        .expect("local-path install");
    assert_eq!(notice.message, SECURITY_CAVEAT, "security caveat surfaced (R-09-019)");

    let listed = mgr.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, rec.id);

    // enable/disable a resource within the package (R-09-018).
    mgr.set_enabled(&rec.id, ResourceSelector::Skill("alpha".to_string()), false).unwrap();
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

    let default_src = PackageSource::Git { url: "file:///x/a".into(), reff: PinRef::Default };
    let tag_src = PackageSource::Git { url: "file:///x/b".into(), reff: PinRef::Tag("v1".into()) };
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
    let report = mgr.update(UpdateTarget::All, CancelToken::new()).await.unwrap();
    assert!(report.skipped_pinned.contains(&tag_id), "bulk update skips pinned (R-09-020)");
    assert!(report.updated.contains(&default_id), "bulk update moves unpinned");
    assert!(!report.updated.contains(&tag_id));

    // update(One) moves the pinned package regardless.
    let one = mgr.update(UpdateTarget::One(tag_id.clone()), CancelToken::new()).await.unwrap();
    assert!(one.updated.contains(&tag_id), "explicit update(One) moves a pinned package");
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
            PackageSource::Git { url: repo_dir.display().to_string(), reff: PinRef::Default },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("install from local git repo");
    assert!(rec.resolved_commit.is_some(), "HEAD commit resolved via gix (no network)");
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
        .install(PackageSource::Path { path: pkg.clone() }, InstallScope::Project, false, CancelToken::new())
        .await
        .expect_err("untrusted project install must be refused");
    assert!(matches!(err, cyrup_resources::ResourceError::Untrusted(_)));

    // Trusted project install succeeds and still surfaces the security caveat (R-09-019).
    let (_rec, notice) = mgr
        .install(PackageSource::Path { path: pkg }, InstallScope::Project, true, CancelToken::new())
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
    let skill = report.registry.skills.get_name("contrib").expect("contributed skill available");
    assert_eq!(skill.scope, ResourceScope::Discovered, "enters at Discovered scope (R-09-022)");
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
    write(&root.join(".cyrup/skills/late/SKILL.md"), &skill_md("late", "added at runtime"));
    let second = run_discover(&c).await;
    handle.store(std::sync::Arc::new(second.registry));
    assert!(handle.load().skills.contains("late"), "/reload re-discovers new skill");
}

// ===========================================================================
// A-09-9 — same-name precedence (built-in -> global -> project, later wins)
// ===========================================================================

#[tokio::test]
async fn a09_9_same_name_precedence_project_beats_global() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("global/skills/dup/SKILL.md"), &skill_md("dup", "global one"));
    write(&root.join(".cyrup/skills/dup/SKILL.md"), &skill_md("dup", "project one"));

    let mut c = cfg(root);
    c.trusted_project = true;
    let report = run_discover(&c).await;

    let winner = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(winner.scope, ResourceScope::Project, "project wins over global (R-09-024)");
    assert_eq!(winner.front.description.as_deref(), Some("project one"));

    // Both candidates retained for diagnostics.
    let count = report.registry.skills.all().iter().filter(|s| s.key.as_str() == "dup").count();
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
    write(&p, "---\ndescription: use when frobbing\ndisable-model-invocation: true\n---\nBody.\n");

    let (skill, diags) =
        Skill::load_with_diagnostics(&p, ResourceScope::Cli, cyrup_resources::ResourceOrigin::Builtin)
            .unwrap();
    let skill = skill.expect("skill loads with a parent-dir name");
    assert_eq!(skill.name, "my-skill");
    assert!(skill.disable_model_invocation, "disable-model-invocation parsed (skills.ts:316)");
    assert!(diags.is_empty(), "valid skill yields no diagnostics");
}

#[test]
fn skill_missing_description_is_dropped_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("foo/SKILL.md");
    write(&p, "---\nname: foo\n---\nBody only, no description.\n");

    let (skill, diags) =
        Skill::load_with_diagnostics(&p, ResourceScope::Cli, cyrup_resources::ResourceOrigin::Builtin)
            .unwrap();
    assert!(skill.is_none(), "no-description skill is dropped (skills.ts:305-307)");
    assert!(
        diags.iter().any(|d| d.message.contains("description is required")),
        "warning diagnostic emitted for missing description"
    );
}

#[test]
fn skill_invalid_name_warns_but_still_loads() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("x/SKILL.md");
    // Uppercase + underscore violate the name charset, but a valid description keeps the skill.
    write(&p, "---\nname: Bad_Name\ndescription: use when testing\n---\nBody.\n");

    let (skill, diags) =
        Skill::load_with_diagnostics(&p, ResourceScope::Cli, cyrup_resources::ResourceOrigin::Builtin)
            .unwrap();
    assert!(skill.is_some(), "skill kept despite invalid name (skills.ts: still load with warnings)");
    assert!(diags.iter().any(|d| d.message.contains("invalid characters")));

    // Direct validator checks (skills.ts:92-112).
    assert!(validate_name("good-name").is_empty());
    assert!(!validate_name("-bad").is_empty(), "leading hyphen invalid");
    assert!(!validate_name("a--b").is_empty(), "consecutive hyphens invalid");
}

#[tokio::test]
async fn skill_name_collision_emits_collision_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("global/skills/dup/SKILL.md"), &skill_md("dup", "global one"));
    write(&root.join(".cyrup/skills/dup/SKILL.md"), &skill_md("dup", "project one"));

    let mut c = cfg(root);
    c.trusted_project = true;
    let report = run_discover(&c).await;

    let collision = report
        .diagnostics
        .iter()
        .find(|d| d.diagnostic_type == DiagnosticType::Collision)
        .expect("a collision diagnostic is emitted (skills.ts:410-427)");
    let detail = collision.collision.as_ref().expect("collision detail present");
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
        Some(&ColorSpec::Rgb { r: 0x0a, g: 0x14, b: 0x1e })
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
    let dark = report.registry.themes.get_name("dark").expect("dark present");
    assert_eq!(dark.scope, ResourceScope::Global, "global theme overrides built-in");
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
    assert!(msg.contains("Missing required color tokens"), "Pi error text: {msg}");
    // A representative token from each schema section is reported as missing.
    for token in ["syntaxKeyword", "mdHeading", "thinkingHigh", "bashMode"] {
        assert!(msg.contains(token), "missing token `{token}` listed: {msg}");
    }
    // The present token is NOT reported as missing.
    assert!(!msg.contains("- accent\n"), "provided token must not be flagged: {msg}");

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
    let dark = builtins.iter().find(|t| t.data.name == "dark").expect("dark builtin");

    // Every required token resolves (no incomplete role map) — the gap that left cyrup-tui unable
    // to render (theme.rs:276-307 stub had only 4 non-Pi tokens).
    let resolved = dark.resolve();
    for token in cyrup_resources::REQUIRED_COLOR_TOKENS {
        assert!(resolved.roles.contains_key(token), "dark resolves `{token}`");
    }
    // A var-indirected token and a literal-hex token resolve to the Pi values.
    assert_eq!(
        resolved.roles.get("syntaxKeyword"),
        Some(&ColorSpec::Rgb { r: 0x56, g: 0x9c, b: 0xd6 }),
        "syntaxKeyword = #569CD6 (literal hex from dark.json)"
    );
    assert_eq!(
        resolved.roles.get("success"),
        Some(&ColorSpec::Rgb { r: 0xb5, g: 0xbd, b: 0x68 }),
        "success -> $green -> #b5bd68 (var indirection)"
    );

    // Typed export section resolves for HTML export (theme.ts:94-100; G10).
    let export = dark.resolve_export();
    assert_eq!(export.page_bg, ColorSpec::Rgb { r: 0x18, g: 0x18, b: 0x1e });
    assert_eq!(export.card_bg, ColorSpec::Rgb { r: 0x1e, g: 0x1e, b: 0x24 });
    assert_eq!(export.info_bg, ColorSpec::Rgb { r: 0x3c, g: 0x37, b: 0x28 });

    let light = builtins.iter().find(|t| t.data.name == "light").expect("light builtin");
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
    write(&skills_root.join("secret/hidden/SKILL.md"), &skill_md("hidden", "should be ignored"));
    write(&skills_root.join("visible/SKILL.md"), &skill_md("visible", "should load"));

    let c = cfg(root);
    let report = run_discover(&c).await;
    assert!(report.registry.skills.contains("visible"), "non-ignored skill loads");
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
    write(&skills_root.join("draft-drop/SKILL.md"), &skill_md("draftdrop", "ignored skill"));
    write(&skills_root.join("draft-keep/SKILL.md"), &skill_md("draftkeep", "re-included skill"));

    let c = cfg(root);
    let report = run_discover(&c).await;
    assert!(!report.registry.skills.contains("draftdrop"), "draft-drop ignored by glob");
    assert!(report.registry.skills.contains("draftkeep"), "draft-keep re-included via negation");
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
    assert!(m.themes.iter().all(|p| p.extension().is_some_and(|e| e == "json")));
}

#[tokio::test]
async fn manifest_glob_themes_surface_in_discovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pkg_dir = root.join("pkgtree");
    write(&pkg_dir.join("themes/midnight.json"), &full_theme_json("midnight", &[], &[]));
    write(&pkg_dir.join("themes/noon.json"), &full_theme_json("noon", &[], &[]));
    write(
        &pkg_dir.join("package.json"),
        r#"{"name":"pack","pi":{"themes":["themes/*.json"]}}"#,
    );

    let mut c = cfg(root);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path { path: pkg_dir.clone() },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(report.registry.themes.contains("midnight"), "glob-matched theme file surfaces");
    assert!(report.registry.themes.contains("noon"), "second glob-matched theme file surfaces");
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
    write(&skills_root.join("real/SKILL.md"), &skill_md("dup", "the one true skill"));
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
            PackageSource::Git { url: repo_dir.display().to_string(), reff: PinRef::Tag("v1".into()) },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("install pinned to tag v1 from a local git repo");

    // The store working copy must reflect the *pinned* ref, not default HEAD: marker.txt held "v1"
    // at the tagged commit and "v2" at HEAD. Locate the materialized clone via the store dir.
    let store_dir = store.package_dir(InstallScope::Global, &rec.id).expect("package dir");
    let marker = store_dir.join("marker.txt");
    assert!(marker.exists(), "checked-out tree present at {}", store_dir.display());
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
