//! The bundled operational skill this extension ships (ICOM-004): `skills/pi-intercom/SKILL.md`,
//! a port of `pi-intercom` **v0.10.1** `skills/pi-intercom/SKILL.md` (452 lines).
//!
//! pi declares it in its `package.json` `pi` block — `"pi": { "skills": ["./skills"] }`
//! (`package.json:26-28`) — and pi's resource discovery loads it when the extension is installed.
//! cyrup's analog is `cyrup-resources`' manifest auto-discovery: a resource root with conventional
//! `agents/`, `prompts/` and `skills/` children is recognized WITHOUT any `cyrup.toml`, and
//! [`cyrup_resources::resolve_manifest`] returns each present child in the matching
//! [`cyrup_resources::ResolvedManifest`] field (`package/manifest.rs:94-102`).
//!
//! This module is the registration seam, and is deliberately the same seam
//! `cyrup-ext-subagents` already uses (`registration/resources.rs`) rather than a second mechanism:
//! it points that resolution at this crate's bundled `resources/` root, expands the manifest's
//! `skills` directory entry into concrete files with pi's own `SKILL.md`-then-`.md` walk (the
//! file-level expansion only runs inside `cyrup-resources`' full discovery pipeline, so a caller
//! that wants the shipped artifacts has to reproduce it), and hands the paths to the
//! [`cyrup_ext::EventKind::ResourcesDiscover`] answer in [`crate::extension::IntercomExtension`].

use std::path::{Path, PathBuf};

use cyrup_resources::{ResolvedManifest, resolve_manifest};

/// Environment override for the bundled-resources root, mirroring
/// `cyrup-ext-subagents`' `CYRUP_SUBAGENT_BUILTIN_AGENTS_DIR`: a packaged/installed binary that does
/// not ship an intact `CARGO_MANIFEST_DIR`-relative source tree points this at the fixed
/// install-time location the `skills/` directory was vendored into.
///
/// Declared here rather than in [`crate::identity`] — which is this crate's inventory of the env
/// vars that are PORTS of pi variables — because this one has no upstream counterpart at all
/// (pi resolves its skills relative to the installed npm package). It is read once, at discovery.
pub const ENV_INTERCOM_RESOURCES_DIR: &str = "CYRUP_INTERCOM_RESOURCES_DIR";

/// The bundled resources root — the parent of the conventional `skills/` child directory
/// (`crates/cyrup-intercom/resources/`). Overridable via [`ENV_INTERCOM_RESOURCES_DIR`]; defaults
/// to this crate's own `CARGO_MANIFEST_DIR`-relative `resources/` (correct for every from-source
/// build).
#[must_use]
pub fn bundled_resources_dir() -> PathBuf {
    std::env::var_os(ENV_INTERCOM_RESOURCES_DIR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
}

/// Resolve the bundled resources root's manifest (auto-discovery of the conventional children).
/// `None` when the root has no recognizable manifest shape at all (e.g. it does not exist, as in a
/// relocated install whose override is unset).
#[must_use]
pub fn bundled_resource_manifest() -> Option<ResolvedManifest> {
    resolve_manifest(&bundled_resources_dir()).ok()
}

/// Every bundled skill file (`skills/**/SKILL.md`), discovered through the manifest's `skills`
/// entries via pi's `SKILL.md`-then-`.md` walk. Sorted for deterministic output; empty when no
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

/// Expand one manifest `skills` entry into concrete skill files: a directory containing `SKILL.md`
/// yields just that file and stops; otherwise root-level `.md` files are skills and subdirectories
/// are recursed (`cyrup_resources`' `collect_skill_files`, `package/manifest.rs:221-246`).
fn expand_skill_entry(entry: &Path, out: &mut Vec<PathBuf>) {
    if entry.is_file() {
        out.push(entry.to_path_buf());
        return;
    }
    collect_skill_files(entry, true, out);
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
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    name.starts_with('.') || name == "node_modules"
}

/// Whether `path`'s extension equals `ext` (case-sensitive, matching `cyrup_resources`).
fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e == ext)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    /// ICOM-004 — the shipped skill exists and is discoverable through the SAME manifest plumbing
    /// `cyrup-ext-subagents` uses. Pre-fix this whole module did not exist and
    /// `find crates/cyrup-intercom -type f ! -name '*.rs'` returned only `Cargo.toml`.
    #[test]
    fn the_bundled_intercom_skill_is_discoverable() {
        let files = bundled_skill_files();
        assert_eq!(files.len(), 1, "exactly one bundled skill ships, got {files:?}");
        let skill = &files[0];
        assert!(skill.is_file(), "discovered skill {skill:?} must exist on disk");
        assert_eq!(skill.file_name().and_then(|n| n.to_str()), Some("SKILL.md"));
        assert_eq!(
            skill.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()),
            Some("pi-intercom"),
            "the skill keeps upstream's name, as the ported `pi-subagents` skill does"
        );
    }

    /// The shipped text is the v0.10.1 port, not a paraphrase: its front matter carries pi's own
    /// `name:`, and the two forced divergences (ICOM-004's `[CYRUP-DELTA]`) are BOTH applied —
    /// pi's `PI_INTERCOM_ASK_TIMEOUT_MS` is gone in favour of the variable this build actually
    /// reads, and every parameter the body names is advertised by `parameters_schema()`.
    ///
    /// ICOM-042 INVERTED the second half of this. It used to assert `openProjectPaneIfMissing` was
    /// absent, because the schema did not advertise it; the launcher has since landed, so the
    /// skill documents it and the schema must carry it. The intent never changed — the skill must
    /// not tell the model to pass a parameter this build does not accept — so the check is now
    /// expressed AGAINST THE SCHEMA rather than against a hard-coded name, and it cannot go stale
    /// in either direction again.
    #[test]
    fn the_shipped_skill_documents_only_actions_and_env_vars_this_build_honours() {
        let files = bundled_skill_files();
        let text = std::fs::read_to_string(&files[0]).expect("the bundled skill is readable");
        assert!(text.starts_with("---\n"), "front matter is first");
        assert!(text.contains("\nname: pi-intercom\n"), "upstream skill name is preserved");
        // Both forced deltas are asserted against the BODY — the model-facing text — because the
        // `[CYRUP-DELTA]` note in the YAML front matter necessarily quotes what it replaced.
        let body = text.split_once("\n---\n").map(|(_, b)| b).unwrap_or(&text);
        assert!(
            !body.contains("PI_INTERCOM_ASK_TIMEOUT_MS"),
            "pi's env spelling must not be documented: this build reads {}",
            crate::identity::ENV_INTERCOM_ASK_TIMEOUT_MS
        );
        assert!(
            body.contains(crate::identity::ENV_INTERCOM_ASK_TIMEOUT_MS),
            "the ask-timeout override this build honours is documented"
        );
        // Every action the skill tells the model to call must be in the tool's own enum, and every
        // PARAMETER it names must be advertised too. The parameter half is schema-driven rather
        // than a name list, so it stays true whichever way the schema moves next.
        let schema = crate::tools::intercom::parameters_schema();
        let properties = schema["properties"]
            .as_object()
            .expect("the schema is an object with properties");
        for param in ["openProjectPaneIfMissing", "focus", "cwd", "to", "message"] {
            if body.contains(param) {
                assert!(
                    properties.contains_key(param),
                    "the skill documents `{param}`, which `parameters_schema()` does not advertise \
                     — a model told to pass it would have the key silently dropped by \
                     `IntercomParams` (which carries no `deny_unknown_fields`), getting neither \
                     the effect nor an error"
                );
            }
        }
        let advertised: Vec<String> = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("the action property carries an enum")
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        for action in ["send", "ask", "reply", "pending", "list", "status", "list-cwd"] {
            assert!(
                body.contains(&format!("\"{action}\"")),
                "the skill exercises {action:?}, which must be advertised"
            );
            assert!(
                advertised.iter().any(|a| a == action),
                "the skill documents {action:?}, which the tool does not advertise: {advertised:?}"
            );
        }
    }
}
