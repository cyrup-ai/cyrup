//! Shared tempdir / package-tree / local-git-repo builders for the resources conformance suite.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{DiscoveryConfig, discover};
use cyrup_core::CancelToken;

pub(super) fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

pub(super) fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nDo the thing.\n")
}

/// Build a schema-complete theme JSON (every one of the 51 required color tokens present) so it
/// passes Pi's required-token validation (theme.ts:34-93). `vars`/`colors` override defaults; a
/// purely numeric color value is emitted as a JSON integer (256-color index), else as a string.
/// Override keys that are not required tokens (e.g. arbitrary roles) are appended verbatim.
pub(super) fn full_theme_json(
    name: &str,
    vars: &[(&str, &str)],
    colors: &[(&str, &str)],
) -> String {
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
pub(super) fn cfg(root: &Path) -> DiscoveryConfig {
    let global = root.join("global");
    fs::create_dir_all(&global).unwrap();
    let mut c = DiscoveryConfig::new(root, &global);
    c.project_root = Some(root.to_path_buf());
    c.cwd = root.to_path_buf();
    c
}

pub(super) async fn run_discover(c: &DiscoveryConfig) -> crate::DiscoveryReport {
    discover(c, CancelToken::new()).await.unwrap()
}

pub(super) fn make_package_tree(dir: &Path, with_manifest: bool, pi_key: bool) {
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

/// Create a real local git repo with one commit. Returns the tempdir (kept alive) + repo path, or
/// None if the `git` CLI is unavailable.
pub(super) fn make_local_git_repo() -> Option<(tempfile::TempDir, PathBuf)> {
    let tmp = tempfile::tempdir().ok()?;
    let dir = tmp.path().to_path_buf();
    make_package_tree(&dir, true, false);
    if !git_in(&dir, &["init", "-q"]) {
        return None;
    }
    git_in(&dir, &["config", "user.email", "t@t"]);
    git_in(&dir, &["config", "user.name", "t"]);
    if !git_in(&dir, &["add", "-A"]) {
        return None;
    }
    if !git_in(&dir, &["commit", "-q", "-m", "init"]) {
        return None;
    }
    Some((tmp, dir))
}

/// Local git repo with two commits: commit 1 (`marker.txt`=="v1") tagged `v1`, commit 2 sets it to
/// "v2" and is HEAD. Returns None when the `git` CLI is unavailable.
pub(super) fn make_local_git_repo_two_commits() -> Option<(tempfile::TempDir, PathBuf)> {
    let tmp = tempfile::tempdir().ok()?;
    let dir = tmp.path().to_path_buf();
    make_package_tree(&dir, true, false);
    if !git_in(&dir, &["init", "-q"]) {
        return None;
    }
    git_in(&dir, &["config", "user.email", "t@t"]);
    git_in(&dir, &["config", "user.name", "t"]);
    fs::write(dir.join("marker.txt"), "v1\n").ok()?;
    if !git_in(&dir, &["add", "-A"]) || !git_in(&dir, &["commit", "-q", "-m", "c1"]) {
        return None;
    }
    if !git_in(&dir, &["tag", "v1"]) {
        return None;
    }
    fs::write(dir.join("marker.txt"), "v2\n").ok()?;
    if !git_in(&dir, &["add", "-A"]) || !git_in(&dir, &["commit", "-q", "-m", "c2"]) {
        return None;
    }
    Some((tmp, dir))
}

/// Run a `git` subcommand in `dir`; returns false on failure or if the CLI is unavailable.
pub(super) fn git_in(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
