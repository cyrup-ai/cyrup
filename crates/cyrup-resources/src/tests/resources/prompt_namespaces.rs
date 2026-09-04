//! Namespaced prompt templates — recursive prompt scan, skip rules, depth cap, symlink policy.
//! [CYRUP-DELTA] over Pi's flat scan; the governing spec citation is on the banner below.

use std::fs;
use std::path::Path;

use super::fixtures::{cfg, run_discover, write};
use crate::{
    InstallScope, InstalledPackage, InstalledPackages, PackageSource, ResourceScope,
    expand_prompt_template,
};

// ===========================================================================
// Namespaced prompt templates (spec/namespaced-prompt-templates.md) — recursive
// prompt scan: subdirectories become command namespaces (`flux/new.md` under a
// root -> `/flux/new`). [CYRUP-DELTA] Pi is flat/non-recursive
// (prompt-templates.ts:108,136); semantics mirror code-puppy's
// `_command_name_from_path` / `_is_in_skipped_namespace`.
// ===========================================================================

/// §3.1 name derivation through discovery: root-relative path, components joined with `/`,
/// `.md` stripped, case preserved; top-level files keep their flat basename (§7 compat);
/// `/flux/new …` expands through `expand_prompt_template` (the DoD-6 smoke, pinned).
#[tokio::test]
async fn npt_namespaced_names_case_and_expansion() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("global/prompts/review.md"),
        "---\ndescription: flat\n---\nReview $1.\n",
    );
    write(
        &root.join("global/prompts/flux/new.md"),
        "---\ndescription: make a new thing\n---\nSay $1 to the world.\n",
    );
    write(
        &root.join("global/prompts/flux/sub/thing.md"),
        "Thing $1.\n",
    );
    write(&root.join("global/prompts/flux/CamelCase.md"), "CC.\n");
    write(
        &root.join("global/prompts/flux-new.md"),
        "Distinct from flux/new.\n",
    );

    let c = cfg(root);
    let report = run_discover(&c).await;
    let prompts = &report.registry.prompts;

    // §7 compat: a top-level file relativizes to its own basename.
    assert_eq!(
        prompts.get_name("review").expect("flat review").name,
        "review"
    );
    // Namespaced names: segments joined with `/`, case preserved.
    let t = prompts.get_name("flux/new").expect("flux/new discovered");
    assert_eq!(t.name, "flux/new");
    assert_eq!(t.description, "make a new thing");
    assert_eq!(
        prompts
            .get_name("flux/sub/thing")
            .expect("deeper nesting")
            .name,
        "flux/sub/thing"
    );
    assert_eq!(
        prompts
            .get_name("flux/CamelCase")
            .expect("case preserved")
            .name,
        "flux/CamelCase"
    );
    // §3.5: `flux/new` and `flux-new` are DISTINCT keys and coexist.
    assert!(prompts.contains("flux/new"));
    assert!(prompts.contains("flux-new"));

    // The DoD-6 smoke as a pinned test: `/flux/new hello` expands to the substituted body.
    let all: Vec<_> = prompts.winners().cloned().collect();
    assert_eq!(
        expand_prompt_template("/flux/new hello", all.iter()),
        "Say hello to the world."
    );
    // `/name` matching stays case-sensitive on the command spelling (prompt-templates.ts:268-284).
    assert_eq!(
        expand_prompt_template("/Flux/New hello", all.iter()),
        "/Flux/New hello"
    );
}

/// §3.2 skip rules: no descent into `.`-/`_`-prefixed dirs or `node_modules` — at any depth —
/// while FILES with those prefixes in a non-skipped directory still load with literal names.
#[tokio::test]
async fn npt_skip_rules_dir_names_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Skipped directories, at the root and nested inside a scanned one.
    write(&root.join("global/prompts/_docs/a.md"), "no\n");
    write(&root.join("global/prompts/.hidden/b.md"), "no\n");
    write(&root.join("global/prompts/node_modules/c.md"), "no\n");
    write(&root.join("global/prompts/ok/_skip/x.md"), "no\n");
    write(&root.join("global/prompts/ok/node_modules/y.md"), "no\n");
    write(&root.join("global/prompts/ok/.dotdir/z.md"), "no\n");
    // The predicate inspects DIRECTORY names only: these files load with literal names.
    write(&root.join("global/prompts/_leaf.md"), "yes\n");
    write(&root.join("global/prompts/.dot.md"), "yes\n");
    write(&root.join("global/prompts/ok/fine.md"), "yes\n");

    let c = cfg(root);
    let report = run_discover(&c).await;
    let prompts = &report.registry.prompts;

    assert_eq!(
        prompts.get_name("_leaf").expect("_leaf file loads").name,
        "_leaf"
    );
    assert_eq!(
        prompts.get_name(".dot").expect(".dot file loads").name,
        ".dot"
    );
    assert_eq!(
        prompts.get_name("ok/fine").expect("ok/fine loads").name,
        "ok/fine"
    );
    assert_eq!(
        prompts.len(),
        3,
        "nothing from a skipped dir leaks: {:?}",
        prompts.all().iter().map(|t| &t.name).collect::<Vec<_>>()
    );
}

/// §3.3 depth cap: the root is depth 0, a directory at depth 8 is still scanned, its
/// subdirectories are refused — each refused directory producing EXACTLY ONE warning
/// (`namespace depth exceeds 8`), never a silent skip.
#[tokio::test]
async fn npt_depth_cap_warns_once_per_refused_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let deep8 = root.join("global/prompts/d1/d2/d3/d4/d5/d6/d7/d8");
    write(&deep8.join("ok.md"), "deep but legal\n");
    write(&deep8.join("d9a/leaf.md"), "too deep\n");
    write(&deep8.join("d9b/leaf.md"), "too deep\n");

    let c = cfg(root);
    let report = run_discover(&c).await;
    let prompts = &report.registry.prompts;

    // A file eight segments down still loads with the full namespace.
    assert_eq!(
        prompts
            .get_name("d1/d2/d3/d4/d5/d6/d7/d8/ok")
            .expect("depth-8 file loads")
            .name,
        "d1/d2/d3/d4/d5/d6/d7/d8/ok"
    );
    // Depth-9 directories are refused, so their files never register.
    assert!(!prompts.contains("d1/d2/d3/d4/d5/d6/d7/d8/d9a/leaf"));
    assert!(!prompts.contains("d1/d2/d3/d4/d5/d6/d7/d8/d9b/leaf"));

    let refused: Vec<_> = report
        .warnings
        .iter()
        .filter(|w| w.reason == "namespace depth exceeds 8")
        .collect();
    assert_eq!(
        refused.len(),
        2,
        "exactly one warning per refused directory: {:?}",
        report
            .warnings
            .iter()
            .map(|w| (&w.path, &w.reason))
            .collect::<Vec<_>>()
    );
    assert!(
        refused
            .iter()
            .all(|w| matches!(w.kind, crate::ResourceKind::Prompt))
    );
    assert!(refused.iter().any(|w| w.path.ends_with("d9a")));
    assert!(refused.iter().any(|w| w.path.ends_with("d9b")));
}

/// §3.3 symlinks: classified with `symlink_metadata` — directory symlinks are NEVER followed
/// (a self-referential link cannot hang the scan); file symlinks load when the target is a
/// regular `.md` file, under the LINK's root-relative name; broken links skip silently.
#[tokio::test]
async fn npt_symlink_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let prompts_root = root.join("global/prompts");
    write(&prompts_root.join("real/inner.md"), "real\n");
    write(&root.join("global/target.txt"), "not markdown\n");

    // A file symlink to a regular `.md` file: followed (Pi parity, prompt-templates.ts:150-160).
    std::os::unix::fs::symlink(
        prompts_root.join("real/inner.md"),
        prompts_root.join("linkfile.md"),
    )
    .unwrap();
    // Directory symlinks: never recursed — including one pointing back at the scan root.
    std::os::unix::fs::symlink(prompts_root.join("real"), prompts_root.join("linkdir")).unwrap();
    std::os::unix::fs::symlink(&prompts_root, prompts_root.join("loopy")).unwrap();
    // A broken link, and a link whose OWN name lacks the `.md` extension: both skipped.
    std::os::unix::fs::symlink(
        prompts_root.join("missing.md"),
        prompts_root.join("broken.md"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        root.join("global/target.txt"),
        prompts_root.join("wrongext"),
    )
    .unwrap();

    let c = cfg(root);
    let report = run_discover(&c).await;
    let prompts = &report.registry.prompts;

    assert_eq!(
        prompts
            .get_name("linkfile")
            .expect("file symlink followed")
            .name,
        "linkfile"
    );
    assert_eq!(
        prompts
            .get_name("real/inner")
            .expect("real file loads")
            .name,
        "real/inner"
    );
    assert!(
        !prompts.contains("linkdir/inner"),
        "dir symlink never followed"
    );
    assert!(
        !prompts.contains("loopy/real/inner"),
        "self-loop link never followed"
    );
    assert!(!prompts.contains("broken"), "broken link skipped");
    assert!(
        !prompts.contains("wrongext"),
        "extension judged by the link's own name"
    );
    assert_eq!(
        prompts.len(),
        2,
        "only the real file and the followed file link load: {:?}",
        prompts.all().iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    assert!(
        report.warnings.is_empty(),
        "skipped links stay silent: {:?}",
        report.warnings
    );
}

/// §3.1 / §4.1 unit edges for the single load path: `load` delegates to `load_with_root`
/// (basename falls out of the same code path); the stem is `file_name` + `strip_suffix`, NOT
/// `Path::file_stem`; empty stems and non-UTF-8 components keep the existing Manifest error;
/// frontmatter `name:` remains ignored.
#[test]
fn npt_load_with_root_derivation_edges() {
    use crate::{PromptTemplate, ResourceError, ResourceOrigin};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Root-aware derivation: a nested file namespaces against `root`.
    write(&root.join("flux/new.md"), "Body $1\n");
    let t = PromptTemplate::load_with_root(
        &root.join("flux/new.md"),
        root,
        ResourceScope::Cli,
        ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(t.name, "flux/new");

    // `load` relativizes against the file's own parent — Pi's basename behavior exactly.
    let t = PromptTemplate::load(
        &root.join("flux/new.md"),
        ResourceScope::Cli,
        ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(t.name, "new");

    // Stem = `file_name` + `strip_suffix(".md")`: `foo.bar.md` -> `foo.bar`.
    write(&root.join("foo.bar.md"), "x\n");
    let t = PromptTemplate::load(
        &root.join("foo.bar.md"),
        ResourceScope::Cli,
        ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(t.name, "foo.bar");

    // A file literally named `.md` has an empty stem -> the same Manifest error as the old
    // basename derivation (`file_stem` would wrongly yield `.md` here — dotfiles have no ext).
    write(&root.join(".md"), "x\n");
    let err = PromptTemplate::load(
        &root.join(".md"),
        ResourceScope::Cli,
        ResourceOrigin::Builtin,
    )
    .unwrap_err();
    match err {
        ResourceError::Manifest(m) => {
            assert!(m.contains("prompt template has no usable name"), "{m}")
        }
        other => panic!("expected Manifest error, got {other:?}"),
    }

    // Frontmatter `name:` remains ignored — the invocation spelling comes from the layout.
    write(
        &root.join("flux/named.md"),
        "---\nname: other\ndescription: d\n---\nBody\n",
    );
    let t = PromptTemplate::load_with_root(
        &root.join("flux/named.md"),
        root,
        ResourceScope::Cli,
        ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(t.name, "flux/named");

    // A non-ancestor `root` hits the defensive `unwrap_or(path)` fallback: it must not fail.
    let t = PromptTemplate::load_with_root(
        &root.join("flux/new.md"),
        Path::new("/no/such/root"),
        ResourceScope::Cli,
        ResourceOrigin::Builtin,
    )
    .unwrap();
    assert!(!t.name.is_empty());
}

/// §3.1 rule 2: a non-UTF-8 component — leaf OR intermediate dir — maps to the same Manifest
/// error as an empty stem, never a silently dropped component. Linux-only: macOS rejects
/// non-UTF-8 filenames at the VFS layer (`EILSEQ`), so the fixture — and thus the whole
/// scenario — cannot exist there (the OS enforces the rule before the loader ever runs).
#[cfg(target_os = "linux")]
#[test]
fn npt_load_with_root_non_utf8_components_error() {
    use crate::{PromptTemplate, ResourceError, ResourceOrigin};
    use std::os::unix::ffi::OsStrExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let bad_leaf = root.join(std::ffi::OsStr::from_bytes(b"bad\xff.md"));
    fs::write(&bad_leaf, "x\n").unwrap();
    assert!(matches!(
        PromptTemplate::load(&bad_leaf, ResourceScope::Cli, ResourceOrigin::Builtin),
        Err(ResourceError::Manifest(_))
    ));

    let bad_dir = root.join(std::ffi::OsStr::from_bytes(b"dir\xff"));
    fs::create_dir_all(&bad_dir).unwrap();
    let bad_nested = bad_dir.join("leaf.md");
    fs::write(&bad_nested, "x\n").unwrap();
    assert!(matches!(
        PromptTemplate::load_with_root(
            &bad_nested,
            root,
            ResourceScope::Cli,
            ResourceOrigin::Builtin
        ),
        Err(ResourceError::Manifest(_))
    ));
}

/// §4.2 `load_one_prompt`: a template that fails to load becomes a warning — the same
/// swallow-and-warn policy the flat scan had — and never aborts the rest of the scan.
#[tokio::test]
async fn npt_load_error_becomes_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("global/prompts/good.md"), "good\n");
    // Invalid UTF-8 CONTENT: `read_to_string` fails -> the load error surfaces as a warning.
    std::fs::write(root.join("global/prompts/bad.md"), b"\xff\xfe not utf8").unwrap();

    let c = cfg(root);
    let report = run_discover(&c).await;

    assert!(
        report.registry.prompts.contains("good"),
        "the scan continues past the unloadable file"
    );
    assert!(!report.registry.prompts.contains("bad"));
    let prompt_warnings: Vec<_> = report
        .warnings
        .iter()
        .filter(|w| matches!(w.kind, crate::ResourceKind::Prompt))
        .collect();
    assert_eq!(
        prompt_warnings.len(),
        1,
        "exactly one warning for the unloadable file: {:?}",
        report.warnings
    );
    assert!(prompt_warnings[0].path.ends_with("bad.md"));
}

/// §3.5 keys & precedence: `flux/new` and `FLUX/NEW` collide case-insensitively; the
/// project-scope candidate shadows the global one and the WINNER keeps its own case and body.
#[tokio::test]
async fn npt_precedence_shadowing_and_case_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("global/prompts/flux/new.md"), "global $1\n");
    write(&root.join(".cyrup/prompts/FLUX/NEW.md"), "project $1\n");

    let mut c = cfg(root);
    c.trusted_project = true;
    let report = run_discover(&c).await;
    let prompts = &report.registry.prompts;

    let winner = prompts
        .get_name("flux/new")
        .expect("one winner under the shared key");
    assert_eq!(winner.name, "FLUX/NEW", "winner keeps its own case");
    assert_eq!(
        winner.scope,
        ResourceScope::Project,
        "project shadows global"
    );
    assert_eq!(winner.expand("body"), "project body\n");
    // Both candidates are retained for diagnostics (`all`), only one wins.
    assert_eq!(prompts.all().len(), 2);
    assert_eq!(prompts.len(), 1);
}

/// §3.4 every scan channel: all four directory-scan call sites produce namespaced names;
/// the three single-file call sites derive basenames exactly as before.
#[tokio::test]
async fn npt_all_directory_and_single_file_call_sites() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // (2) project walk `.cyrup/prompts` — namespaced.
    write(&root.join(".cyrup/prompts/flux/proj.md"), "proj\n");

    // (3) package manifest: a prompts DIRECTORY namespaces; a single FILE stays basename.
    let pkg = root.join("pkg");
    write(&pkg.join("prompts/flux/pkg.md"), "pkg\n");
    write(&pkg.join("solo.md"), "solo\n");
    write(
        &pkg.join("cyrup.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[resources]\nprompts = [\"./prompts\", \"./solo.md\"]\n",
    );

    // (4) `resources_discover` / extension-contributed dir — namespaced.
    let ext = root.join("extprompts");
    write(&ext.join("flux/ext.md"), "ext\n");

    // CLI `--prompt-template`: a DIRECTORY namespaces (same rule as built-in roots); a FILE
    // stays basename.
    let clidir = root.join("cliprompts");
    write(&clidir.join("flux/cli.md"), "cli\n");
    write(&root.join("one.md"), "one\n");

    // Settings `prompts` array file entry — basename.
    write(&root.join("global/extra/sfile.md"), "sfile\n");

    let mut c = cfg(root);
    c.trusted_project = true;
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path { path: pkg.clone() },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    c.extra.prompt_paths.push(ext.clone());
    c.cli.prompts.push(clidir.clone());
    c.cli.prompts.push(root.join("one.md"));
    c.global_overrides
        .prompts
        .push("extra/sfile.md".to_string());

    let report = run_discover(&c).await;
    let prompts = &report.registry.prompts;

    // Directory-scan call sites -> namespaced names.
    // ((1) the global root is covered by `npt_namespaced_names_case_and_expansion`.)
    assert_eq!(
        prompts
            .get_name("flux/proj")
            .expect("project walk namespaced")
            .scope,
        ResourceScope::Project
    );
    assert!(
        prompts.contains("flux/pkg"),
        "package manifest dir namespaced"
    );
    assert_eq!(
        prompts
            .get_name("flux/ext")
            .expect("resources_discover dir namespaced")
            .scope,
        ResourceScope::Discovered
    );
    assert_eq!(
        prompts
            .get_name("flux/cli")
            .expect("cli dir namespaced")
            .scope,
        ResourceScope::Cli
    );
    // Single-file call sites -> basenames.
    assert!(
        prompts.contains("solo"),
        "package manifest single file -> basename"
    );
    assert!(
        !prompts.contains("pkg/solo"),
        "single file never namespaces"
    );
    assert_eq!(
        prompts.get_name("one").expect("cli file -> basename").scope,
        ResourceScope::Cli
    );
    assert_eq!(
        prompts
            .get_name("sfile")
            .expect("settings file entry -> basename")
            .scope,
        ResourceScope::GlobalSettings
    );
}
