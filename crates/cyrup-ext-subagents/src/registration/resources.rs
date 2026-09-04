//! Bundled packaged resources (R-SA-132/134): the 7 prompt-template recipes (`prompts/*.md`) and
//! the `pi-subagents` operational skill (`skills/pi-subagents/SKILL.md`) this extension ships,
//! discovered through the SAME `cyrup-resources` manifest plumbing the 8 builtin agent personas use
//! (`extension.rs::builtin_agents_dir` → [`cyrup_resources::resolve_manifest`]).
//!
//! pi declares these in its `package.json` `pi` block (`"skills": ["./skills"]`,
//! `"prompts": ["./prompts"]`, `package.json:44-53`); when the extension is installed, pi's
//! resource discovery loads them. cyrup's analog is `cyrup-resources`' manifest auto-discovery: a
//! resource root with conventional `agents/`, `prompts/`, and `skills/` child directories is
//! recognized WITHOUT any `cyrup.toml` — [`cyrup_resources::resolve_manifest`] returns each present
//! child directory in the corresponding [`cyrup_resources::ResolvedManifest`] field
//! (`package/manifest.rs:94-102`). This module is the registration seam: it points that resolution
//! at this crate's bundled `resources/` root and exposes the resolved prompt/skill files, mirroring
//! exactly how `discovery::scan_builtin_agents` consumes the same manifest's `agents` field.
//!
//! # Why this crate expands the manifest's directory entries itself
//!
//! In auto-discovery mode (no `cyrup.toml`), `resolve_manifest` returns each conventional child as a
//! single *directory* path (e.g. `resources/prompts`), not its individual files — the file-level
//! expansion (`collectResourceFiles`, `package/manifest.rs:204-269`) only runs inside
//! `cyrup-resources`' own full discovery pipeline when a package is loaded. So this module reproduces
//! that same expansion (prompts: recursive `*.md`; skills: the `SKILL.md`-then-`.md` walk) to yield
//! the concrete, discoverable files — the exact set a real discovery pass over this bundled root
//! would surface — so a caller (and this module's own test) can assert on the shipped artifacts, not
//! merely on the declared directories.

use std::path::{Path, PathBuf};

use cyrup_resources::{ResolvedManifest, resolve_manifest};

/// Environment override for the bundled-resources root (shared with `extension.rs`'s
/// `builtin_agents_dir`): a packaged/installed binary that does not ship an intact
/// `CARGO_MANIFEST_DIR`-relative source tree can point this at the fixed install-time location its
/// `agents/`, `prompts/`, and `skills/` directories were vendored into. Kept identical to the
/// agents override so the whole bundled `resources/` tree relocates together.
const BUNDLED_RESOURCES_DIR_ENV_VAR: &str = "CYRUP_SUBAGENT_BUILTIN_AGENTS_DIR";

/// The bundled resources root — the parent of the conventional `agents/`, `prompts/`, and `skills/`
/// child directories (`crates/cyrup-ext-subagents/resources/`). Overridable via
/// [`BUNDLED_RESOURCES_DIR_ENV_VAR`]; defaults to this crate's own `CARGO_MANIFEST_DIR`-relative
/// `resources/` (correct for every from-source build).
#[must_use]
pub fn bundled_resources_dir() -> PathBuf {
    std::env::var_os(BUNDLED_RESOURCES_DIR_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
}

/// Resolve the bundled resources root's manifest (auto-discovery of `agents/`, `prompts/`,
/// `skills/`), the same [`cyrup_resources::resolve_manifest`] call `discovery::scan_builtin_agents`
/// makes for the `agents` field. Returns `None` if the root has no recognizable manifest shape at
/// all (e.g. the directory does not exist).
#[must_use]
pub fn bundled_resource_manifest() -> Option<ResolvedManifest> {
    resolve_manifest(&bundled_resources_dir()).ok()
}

/// Every bundled prompt-template file (`prompts/**/*.md`), discovered through the manifest's
/// `prompts` entries. Sorted for deterministic output. Empty if no `prompts/` directory is present.
#[must_use]
pub fn bundled_prompt_files() -> Vec<PathBuf> {
    let Some(manifest) = bundled_resource_manifest() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in &manifest.prompts {
        expand_prompt_entry(entry, &mut out);
    }
    out.sort();
    out
}

/// Every bundled skill file (`skills/**/SKILL.md`), discovered through the manifest's `skills`
/// entries via pi's `SKILL.md`-then-`.md` walk. Sorted for deterministic output. Empty if no
/// `skills/` directory is present.
#[must_use]
pub fn bundled_skill_files() -> Vec<PathBuf> {
    let Some(manifest) = bundled_resource_manifest() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in &manifest.skills {
        expand_skill_entry(entry, &mut out);
    }
    out.sort();
    out
}

/// Expand one manifest `prompts` entry into concrete `*.md` files: a file is kept as-is; a directory
/// is recursed for `.md` files (mirrors `cyrup_resources`' `collect_files_with_ext`,
/// `package/manifest.rs:250-269`).
fn expand_prompt_entry(entry: &Path, out: &mut Vec<PathBuf>) {
    if entry.is_file() {
        if has_extension(entry, "md") {
            out.push(entry.to_path_buf());
        }
        return;
    }
    collect_files_with_ext(entry, "md", out);
}

/// Expand one manifest `skills` entry into concrete skill files: a directory containing `SKILL.md`
/// yields just that file and stops; otherwise root-level `.md` files are skills and subdirectories
/// are recursed (mirrors `cyrup_resources`' `collect_skill_files`, `package/manifest.rs:221-246`).
fn expand_skill_entry(entry: &Path, out: &mut Vec<PathBuf>) {
    if entry.is_file() {
        out.push(entry.to_path_buf());
        return;
    }
    collect_skill_files(entry, true, out);
}

/// Recursively collect files with `ext` under `dir`, skipping dot-entries and `node_modules`.
fn collect_files_with_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for path in children {
        if is_skippable_entry(&path) {
            continue;
        }
        if path.is_dir() {
            collect_files_with_ext(&path, ext, out);
        } else if path.is_file() && has_extension(&path, ext) {
            out.push(path);
        }
    }
}

/// pi's `collectSkillEntries(dir, "pi")` walk: a directory containing `SKILL.md` yields that file
/// and stops; otherwise root-level `.md` children are skills and every subdirectory is recursed.
fn collect_skill_files(dir: &Path, root_level: bool, out: &mut Vec<PathBuf>) {
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() {
        out.push(skill_md);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for path in children {
        if is_skippable_entry(&path) {
            continue;
        }
        if path.is_dir() {
            collect_skill_files(&path, false, out);
        } else if root_level && path.is_file() && has_extension(&path, "md") {
            out.push(path);
        }
    }
}

/// A dot-entry or `node_modules` directory the resource walks skip.
fn is_skippable_entry(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    name.starts_with('.') || name == "node_modules"
}

/// Whether `path`'s extension equals `ext` (case-sensitive, matching `cyrup_resources`).
fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e == ext)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    /// The 5 prompt-template recipes pi ships (`prompts/*.md` @ v0.43.0), which this crate mirrors
    /// under `resources/prompts/`. `parallel-context-build` and `parallel-handoff-plan` were deleted
    /// upstream in `83b9872` together with the `planner`/`context-builder` roles they drove — a
    /// recipe whose every step names a role that no longer exists is dead on arrival.
    const EXPECTED_PROMPTS: [&str; 5] = [
        "gather-context-and-clarify",
        "parallel-cleanup",
        "parallel-research",
        "parallel-review",
        "review-loop",
    ];

    fn file_stems(files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
            .collect()
    }

    #[test]
    fn bundled_resources_root_resolves_a_manifest() {
        let manifest = bundled_resource_manifest().expect("bundled resources manifest resolves");
        // The manifest declares all three conventional child directories.
        assert!(!manifest.agents.is_empty(), "agents/ declared");
        assert!(!manifest.prompts.is_empty(), "prompts/ declared");
        assert!(!manifest.skills.is_empty(), "skills/ declared");
    }

    #[test]
    fn all_seven_bundled_prompts_are_present_and_discoverable() {
        let files = bundled_prompt_files();
        let stems = file_stems(&files);
        for expected in EXPECTED_PROMPTS {
            assert!(
                stems.iter().any(|s| s == expected),
                "expected bundled prompt {expected:?} to be discoverable, got {stems:?}"
            );
        }
        assert_eq!(
            files.len(),
            EXPECTED_PROMPTS.len(),
            "exactly the 7 pi prompt recipes ship, got {stems:?}"
        );
        // Every discovered file is a real, readable `.md` on disk.
        for f in &files {
            assert!(f.is_file(), "discovered prompt {f:?} must exist on disk");
            assert!(has_extension(f, "md"));
        }
    }

    #[test]
    fn the_pi_subagents_skill_is_present_and_discoverable() {
        let files = bundled_skill_files();
        assert_eq!(
            files.len(),
            1,
            "exactly one bundled skill (pi-subagents/SKILL.md)"
        );
        let skill = &files[0];
        assert!(skill.ends_with("pi-subagents/SKILL.md"), "got {skill:?}");
        assert!(skill.is_file(), "SKILL.md must exist on disk");
        let contents = std::fs::read_to_string(skill).expect("read SKILL.md");
        assert!(!contents.trim().is_empty(), "SKILL.md is non-empty");
    }
}
