//! The leaf scanners: directory walks, per-file loaders, ignore handling, and the same-name
//! collision diagnostics that [`super::blocking`]'s collectors and [`super::ResourceRegistry`] are
//! built out of. Nothing here reads [`super::DiscoveryConfig`] beyond the resource-kind enable
//! flags handed to it.

use std::path::{Path, PathBuf};

use crate::error::{ResourceDiagnostic, ResourceKind, ResourceWarning};
use crate::key::ResourceKey;
use crate::package::manifest::{ManifestResourceType, resolve_local_entries};
use crate::prompt::PromptTemplate;
use crate::scope::{ResourceOrigin, ResourceScope};
use crate::skill::Skill;
use crate::theme::Theme;

use super::{DiscoveryConfig, Named, ResourceOverrides, ResourceSet};

/// Emit a `collision` diagnostic for every shadowed same-name candidate in `set`.
pub(super) fn emit_collisions<T: Named + Clone>(
    set: &ResourceSet<T>,
    kind: ResourceKind,
    path_of: impl Fn(&T) -> PathBuf,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let mut seen: std::collections::HashSet<(ResourceKey, PathBuf)> =
        std::collections::HashSet::new();
    // Resolve symlinks before comparing so a duplicate reached via a symlink collapses onto the
    // real file rather than surfacing as a spurious collision (skills.ts:403-408 `canonicalizePath`
    // + `realPathSet`). Falls back to the raw path when the file cannot be canonicalized.
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    for candidate in set.all() {
        let Some(winner) = set.get(candidate.key()) else {
            continue;
        };
        let winner_path = path_of(winner);
        let loser_path = path_of(candidate);
        let winner_canon = canon(&winner_path);
        let loser_canon = canon(&loser_path);
        if loser_canon == winner_canon {
            continue; // the winner itself, or the same file reached via a symlink
        }
        if !seen.insert((candidate.key().clone(), loser_canon)) {
            continue; // dedup symlinked duplicates
        }
        diagnostics.push(ResourceDiagnostic::collision(
            kind,
            candidate.key().as_str(),
            winner_path,
            loser_path,
        ));
    }
}

pub(super) fn name_disabled(set: &std::collections::BTreeSet<String>, key: &ResourceKey) -> bool {
    set.iter().any(|n| &ResourceKey::normalize(n) == key)
}

/// Skill discovery rules (skills.ts:150-272), ported 1:1:
/// - if a directory contains `SKILL.md`, treat it as a skill root and do not recurse further;
/// - otherwise load direct `.md` children of the root as skills;
/// - recurse into subdirectories to find `SKILL.md` (loose `.md` children only count at the root).
///
/// `node_modules` and dot-directories are skipped (skills.ts:223,229-231).
pub(super) fn scan_skill_root(
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    scan_skill_dir(
        root,
        scope,
        origin_root,
        true,
        Vec::new(),
        out,
        warnings,
        diagnostics,
    );
}

#[allow(clippy::too_many_arguments)]
fn scan_skill_dir(
    dir: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    include_root_files: bool,
    mut ignore_patterns: Vec<String>,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();

    // Accumulate `.gitignore`/`.ignore`/`.fdignore` rules from this directory, prefixed relative to
    // the scan root, then build a matcher for the rules seen so far (skills.ts:16,47-65,188-189).
    collect_ignore_patterns(dir, origin_root, &mut ignore_patterns);
    let matcher = build_ignore(origin_root, &ignore_patterns);
    let is_ignored = |path: &Path, is_dir: bool| -> bool {
        let Ok(rel) = path.strip_prefix(origin_root) else {
            return false;
        };
        let rel = to_posix(rel);
        if rel.is_empty() {
            return false;
        }
        matcher.matched(&rel, is_dir).is_ignore()
    };

    // First pass: a `SKILL.md` makes this a single skill root — load it and stop (skills.ts:194-220).
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() && !is_ignored(&skill_md, false) {
        load_one_skill(&skill_md, scope, origin_root, out, warnings, diagnostics);
        return;
    }

    // Second pass: direct `.md` children + recurse subdirs (skills.ts:221-269).
    for path in children {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let is_dir = path.is_dir();
        if is_ignored(&path, is_dir) {
            continue;
        }
        if is_dir {
            scan_skill_dir(
                &path,
                scope,
                origin_root,
                false,
                ignore_patterns.clone(),
                out,
                warnings,
                diagnostics,
            );
        } else if include_root_files
            && path.is_file()
            && path.extension().is_some_and(|e| e == "md")
        {
            load_one_skill(&path, scope, origin_root, out, warnings, diagnostics);
        }
    }
}

/// Convert a path to a forward-slash string (skills.ts `toPosixPath`).
fn to_posix(p: &Path) -> String {
    p.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Read this directory's ignore files and append their patterns, prefixed by the directory's path
/// relative to the scan root, so a nested ignore file scopes to its own subtree (skills.ts:47-65).
fn collect_ignore_patterns(dir: &Path, root: &Path, patterns: &mut Vec<String>) {
    let prefix = match dir.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => String::new(),
        Ok(rel) => format!("{}/", to_posix(rel)),
        Err(_) => String::new(),
    };
    for filename in [".gitignore", ".ignore", ".fdignore"] {
        let Ok(content) = std::fs::read_to_string(dir.join(filename)) else {
            continue;
        };
        for line in content.lines() {
            if let Some(pattern) = prefix_ignore_pattern(line, &prefix) {
                patterns.push(pattern);
            }
        }
    }
}

/// Prefix a single ignore pattern with the subdirectory prefix, preserving `!` negation and
/// stripping a leading `/` (1:1 with skills.ts `prefixIgnorePattern`, lines 24-45).
fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }
    let mut pattern = line;
    let mut negated = false;
    if let Some(rest) = pattern.strip_prefix('!') {
        negated = true;
        pattern = rest;
    } else if let Some(rest) = pattern.strip_prefix("\\!") {
        pattern = rest;
    }
    if let Some(rest) = pattern.strip_prefix('/') {
        pattern = rest;
    }
    let prefixed = if prefix.is_empty() {
        pattern.to_string()
    } else {
        format!("{prefix}{pattern}")
    };
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

/// Build a gitignore matcher rooted at the scan root from the accumulated prefixed patterns.
fn build_ignore(root: &Path, patterns: &[String]) -> ignore::gitignore::Gitignore {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    for pattern in patterns {
        let _ = builder.add_line(None, pattern);
    }
    builder
        .build()
        .unwrap_or_else(|_| ignore::gitignore::Gitignore::empty())
}

/// Load the plain-path positive listings from a settings `skills`/`prompts`/`themes` array at the
/// settings tier (`scope`), via [`resolve_local_entries`] (Pi `resolveLocalEntries`,
/// package-manager.ts:2218-2239). The pattern subset of each array has already selected the enabled
/// files; everything returned here is loaded. Resource-kind enable flags are honored.
#[allow(clippy::too_many_arguments)]
pub(super) fn add_local_entries(
    base: &Path,
    overrides: &ResourceOverrides,
    scope: ResourceScope,
    cfg: &DiscoveryConfig,
    skills: &mut Vec<Skill>,
    prompts: &mut Vec<PromptTemplate>,
    themes: &mut Vec<Theme>,
    ext_paths: &mut Vec<PathBuf>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    // `extensions` is the FIRST entry of Pi's `RESOURCE_TYPES` (package-manager.ts:194) and goes
    // through the very same `resolveLocalEntries` pass (:905-931). A settings-declared extension
    // root is therefore LOADED, not just pattern-filtered (CFG-004).
    for e in resolve_local_entries(
        base,
        &overrides.extensions,
        ManifestResourceType::Extensions,
    ) {
        if !ext_paths.contains(&e) {
            ext_paths.push(e);
        }
    }
    if cfg.enable_skills {
        for md in resolve_local_entries(base, &overrides.skills, ManifestResourceType::Skills) {
            let root = md.parent().unwrap_or(&md).to_path_buf();
            load_one_skill(&md, scope, &root, skills, warnings, diagnostics);
        }
    }
    if cfg.enable_prompts {
        for p in resolve_local_entries(base, &overrides.prompts, ManifestResourceType::Prompts) {
            let origin = ResourceOrigin::LooseFile {
                scope,
                root: p.parent().unwrap_or(&p).to_path_buf(),
            };
            match PromptTemplate::load(&p, scope, origin) {
                Ok(t) => prompts.push(t),
                Err(e) => {
                    warnings.push(ResourceWarning::new(ResourceKind::Prompt, p, e.to_string()))
                }
            }
        }
    }
    if cfg.enable_themes {
        for t in resolve_local_entries(base, &overrides.themes, ManifestResourceType::Themes) {
            let origin = ResourceOrigin::LooseFile {
                scope,
                root: t.parent().unwrap_or(&t).to_path_buf(),
            };
            match Theme::load(&t, scope, origin) {
                Ok(th) => themes.push(th),
                Err(e) => {
                    warnings.push(ResourceWarning::new(ResourceKind::Theme, t, e.to_string()))
                }
            }
        }
    }
}

pub(super) fn load_one_skill(
    md: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let origin = ResourceOrigin::LooseFile {
        scope,
        root: origin_root.to_path_buf(),
    };
    match Skill::load_with_diagnostics(md, scope, origin) {
        Ok((skill, diags)) => {
            diagnostics.extend(diags);
            if let Some(s) = skill {
                out.push(s);
            }
        }
        Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Skill, md, e.to_string())),
    }
}

/// Deepest namespace nesting a prompt root will scan (spec/namespaced-prompt-templates.md
/// §3.3). The root itself is depth 0; a directory at depth 8 is still scanned, its
/// subdirectories are refused with a warning.
const MAX_PROMPT_NAMESPACE_DEPTH: usize = 8;

/// Prompt dirs are scanned **recursively** — subdirectories become command namespaces, so
/// `flux/new.md` under a root registers as `/flux/new` (spec/namespaced-prompt-templates.md).
/// [CYRUP-DELTA] Pi's `loadTemplatesFromDir` is non-recursive (prompt-templates.ts:136-174);
/// the recursion and skip rules mirror code-puppy's `_is_in_skipped_namespace`
/// (customizable_commands/register_callbacks.py), plus the skills walker's `node_modules`
/// carve-out.
pub(super) fn scan_prompt_root(
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<PromptTemplate>,
    warnings: &mut Vec<ResourceWarning>,
) {
    scan_prompt_dir(root, root, scope, origin_root, 0, out, warnings);
}

/// Recursive prompt scan. Skip rules: no descent into `.`- or `_`-prefixed dirs or
/// `node_modules`; directory symlinks are never followed (cycle-proof by construction);
/// file symlinks load when the target is a regular `.md` file (Pi's own symlink handling,
/// prompt-templates.ts:150-160). Children are sorted per directory for deterministic
/// first-wins tie-breaking.
fn scan_prompt_dir(
    dir: &Path,
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    depth: usize,
    out: &mut Vec<PromptTemplate>,
    warnings: &mut Vec<ResourceWarning>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();

    for path in children {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let ft = meta.file_type();

        if ft.is_symlink() {
            if path.is_file() && path.extension().is_some_and(|e| e == "md") {
                load_one_prompt(&path, root, scope, origin_root, out, warnings);
            }
            continue;
        }

        if ft.is_dir() {
            if name.starts_with('.') || name.starts_with('_') || name == "node_modules" {
                continue;
            }
            if depth >= MAX_PROMPT_NAMESPACE_DEPTH {
                warnings.push(ResourceWarning::new(
                    ResourceKind::Prompt,
                    path,
                    "namespace depth exceeds 8",
                ));
                continue;
            }
            scan_prompt_dir(&path, root, scope, origin_root, depth + 1, out, warnings);
        } else if ft.is_file() && path.extension().is_some_and(|e| e == "md") {
            load_one_prompt(&path, root, scope, origin_root, out, warnings);
        }
    }
}

/// Load one template, namespaced against `root`; a load error becomes a warning — the same
/// swallow-and-warn policy the flat scan had.
fn load_one_prompt(
    path: &Path,
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<PromptTemplate>,
    warnings: &mut Vec<ResourceWarning>,
) {
    let origin = ResourceOrigin::LooseFile {
        scope,
        root: origin_root.to_path_buf(),
    };
    match PromptTemplate::load_with_root(path, root, scope, origin) {
        Ok(t) => out.push(t),
        Err(e) => warnings.push(ResourceWarning::new(
            ResourceKind::Prompt,
            path,
            e.to_string(),
        )),
    }
}

/// Theme dirs are scanned **non-recursively** — only direct `.json` children (resource-loader.ts:853-881).
pub(super) fn scan_theme_root(
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<Theme>,
    warnings: &mut Vec<ResourceWarning>,
) {
    for json in direct_children(root, "json") {
        let origin = ResourceOrigin::LooseFile {
            scope,
            root: origin_root.to_path_buf(),
        };
        match Theme::load(&json, scope, origin) {
            Ok(t) => out.push(t),
            Err(e) => warnings.push(ResourceWarning::new(
                ResourceKind::Theme,
                json,
                e.to_string(),
            )),
        }
    }
}

/// Direct file children of `dir` with the given extension, sorted. Non-recursive.
fn direct_children(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == ext))
        .collect();
    out.sort();
    out
}

/// Add a skill from an explicit path: a `SKILL.md` file, a skill dir, or a skills root.
pub(super) fn add_skill_path(
    path: &Path,
    scope: ResourceScope,
    origin: ResourceOrigin,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    // An explicitly configured path that does not exist is a diagnostic, not a silent empty scan
    // (skills.ts:457-459: `{ type: "warning", message: "skill path does not exist" }`).
    if !path.exists() {
        diagnostics.push(ResourceDiagnostic::warning(
            ResourceKind::Skill,
            path,
            "skill path does not exist",
        ));
        return;
    }
    let md = if path.is_file() {
        path.to_path_buf()
    } else if path.join("SKILL.md").is_file() {
        path.join("SKILL.md")
    } else {
        // Treat as a root containing multiple skills.
        scan_skill_root(path, scope, path, out, warnings, diagnostics);
        return;
    };
    match Skill::load_with_diagnostics(&md, scope, origin) {
        Ok((skill, diags)) => {
            diagnostics.extend(diags);
            if let Some(s) = skill {
                out.push(s);
            }
        }
        Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Skill, md, e.to_string())),
    }
}

pub(super) fn add_prompt_path(
    path: &Path,
    scope: ResourceScope,
    origin: ResourceOrigin,
    out: &mut Vec<PromptTemplate>,
    warnings: &mut Vec<ResourceWarning>,
) {
    if !path.exists() {
        warnings.push(ResourceWarning::new(
            ResourceKind::Prompt,
            path,
            "prompt path does not exist",
        ));
        return;
    }
    if path.is_dir() {
        scan_prompt_root(path, scope, path, out, warnings);
        return;
    }
    match PromptTemplate::load(path, scope, origin) {
        Ok(t) => out.push(t),
        Err(e) => {
            warnings.push(ResourceWarning::new(
                ResourceKind::Prompt,
                path,
                e.to_string(),
            ));
        }
    }
}

pub(super) fn add_theme_path(
    path: &Path,
    scope: ResourceScope,
    origin: ResourceOrigin,
    out: &mut Vec<Theme>,
    warnings: &mut Vec<ResourceWarning>,
) {
    if !path.exists() {
        warnings.push(ResourceWarning::new(
            ResourceKind::Theme,
            path,
            "theme path does not exist",
        ));
        return;
    }
    if path.is_dir() {
        scan_theme_root(path, scope, path, out, warnings);
        return;
    }
    match Theme::load(path, scope, origin) {
        Ok(t) => out.push(t),
        Err(e) => warnings.push(ResourceWarning::new(
            ResourceKind::Theme,
            path,
            e.to_string(),
        )),
    }
}
