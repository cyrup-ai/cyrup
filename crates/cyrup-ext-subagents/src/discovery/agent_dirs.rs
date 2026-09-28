//! SUBA-123a — the `subagents.agentScanDirs` / `subagents.agentExcludeDirs` directory algebra:
//! extra discovery roots (with one optional wildcard segment) and subtree exclusions applied to
//! every agent-definition directory walk.
//!
//! A direct port of pi-subagents `src/agents/agents.ts` @v0.71.0:
//!
//! | upstream | here |
//! |---|---|
//! | `canonicalAgentPath` (`:2406-2413`) | [`canonical_agent_path`] |
//! | `agentExclusionRoots` (`:2416-2422`) | [`agent_exclusion_roots`] |
//! | `agentExclusions` (`:2424-2431`) | [`is_excluded`] |
//! | `expandAgentScanDirPattern` (`:2433-2458`) | [`expand_agent_scan_dir_pattern`] |
//! | `settingsAgentScanDirs` (`:2460-2470`) | [`settings_agent_scan_dirs`] |
//! | `isPathWithin` (`:2546-2549`) | [`is_path_within`] |
//!
//! **CYRUP-DELTA (watch paths).** Upstream's `AgentScanDirs` carries a `watchPaths` list beside
//! `dirs`, feeding `buildAgentDiscoverySources`' fingerprint/watcher set (`agents.ts:2665-2673`).
//! This port has no agent-discovery watcher — `run_discovery` re-scans on demand per call
//! (R-SA-019) — so only the `dirs` half is modelled. Nothing here would consume a watch path.
//!
//! **CYRUP-DELTA (resolution base).** The two upstream helpers deliberately resolve against
//! DIFFERENT bases and that asymmetry is preserved: an `agentExcludeDirs` entry resolves against
//! `path.dirname(settingsPath)` (`agents.ts:2418`), so it is relative to the settings file that
//! declared it, while an `agentScanDirs` entry resolves against the process cwd via bare
//! `path.resolve` (`agents.ts:2440,2447`). Changing either would silently relocate a user's
//! configured roots.

use std::path::{Component, Path, PathBuf};

/// One resolved `subagents.agentExcludeDirs` entry, in both forms upstream keeps
/// (`agents.ts:2419-2420`): `resolved` is the lexically-resolved path as written, `real` is the
/// same path with every existing ancestor symlink followed ([`canonical_agent_path`]). Both arms
/// are needed — a candidate can sit under the excluded root as WRITTEN, or reach the same subtree
/// through a symlink alias that only matches after canonicalisation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExclusionRoot {
    /// The entry resolved against the directory of the settings file that declared it.
    pub(crate) resolved: PathBuf,
    /// [`canonical_agent_path`] of [`Self::resolved`].
    pub(crate) real: PathBuf,
}

/// Lexically normalise an absolute-or-relative path: drop `.` segments and collapse `..` against
/// the preceding real segment, leaving leading `..`s alone. Node's `path.resolve`/`path.relative`
/// normalise this way without touching the filesystem, and [`is_path_within`] needs the same
/// property so `<root>/a/../b` and `<root>/b` compare equal.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop a real, poppable segment: popping past a root or past a leading `..`
                // would invent a path the caller never wrote.
                let popped = matches!(
                    out.components().next_back(),
                    Some(Component::Normal(_)) | Some(Component::CurDir)
                ) && out.pop();
                if !popped {
                    out.push(Component::ParentDir);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Node's `path.resolve(base, entry)`: an absolute `entry` wins outright, a relative one is joined
/// onto `base`, and the result is lexically normalised.
fn resolve_against(base: &Path, entry: &Path) -> PathBuf {
    lexically_normalize(&base.join(entry))
}

/// Node's single-argument `path.resolve(entry)` — resolve against the process cwd.
///
/// A cwd that cannot be read (deleted working directory) leaves the entry as written rather than
/// failing discovery: an unresolvable relative scan-dir simply matches nothing, which is
/// `expandAgentScanDirPattern`'s own answer for a root that does not exist.
fn resolve_against_cwd(entry: &Path) -> PathBuf {
    if entry.is_absolute() {
        return lexically_normalize(entry);
    }
    match std::env::current_dir() {
        Ok(cwd) => resolve_against(&cwd, entry),
        Err(_) => lexically_normalize(entry),
    }
}

/// pi `isPathWithin` (`agents.ts:2546-2549`): true iff `candidate` IS `root` or sits underneath it.
/// Compared component-wise after lexical normalisation, which is what upstream's
/// `path.relative(...)`-is-not-`..` test computes.
#[must_use]
pub(crate) fn is_path_within(root: &Path, candidate: &Path) -> bool {
    lexically_normalize(candidate).starts_with(lexically_normalize(root))
}

/// pi `canonicalAgentPath` (`agents.ts:2406-2413`) — "resolve existing ancestors to canonicalize
/// missing descendants of symlinks".
///
/// **CYRUP-DELTA.** Node's `fs.realpathSync` throws on a path whose LEAF is missing, which is why
/// upstream recurses onto `path.dirname` and rejoins the basename. `std::fs::canonicalize` has the
/// same all-or-nothing behaviour, so the recursion is reproduced here rather than replaced by a
/// lexical-only resolve: an exclusion root naming a not-yet-created directory under a symlinked
/// parent must still canonicalise to the parent's real location, or the `real` arm of
/// [`is_excluded`] silently stops matching.
#[must_use]
pub(crate) fn canonical_agent_path(path: &Path) -> PathBuf {
    let resolved = resolve_against_cwd(path);
    canonicalize_resolved(&resolved)
}

/// [`canonical_agent_path`]'s recursion, on an already-resolved path.
fn canonicalize_resolved(resolved: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(resolved) {
        return real;
    }
    let Some(parent) = resolved.parent() else {
        return resolved.to_path_buf();
    };
    // Upstream's `parent === resolved` fixed-point guard (`agents.ts:2411`): at a filesystem root
    // `path.dirname` returns the root itself, and the recursion must stop rather than spin.
    let Some(base) = resolved.file_name() else {
        return resolved.to_path_buf();
    };
    if parent == resolved {
        return resolved.to_path_buf();
    }
    canonicalize_resolved(parent).join(base)
}

/// pi `expandHomePath` as applied to these two keys (`agents.ts:2418,2434`). Exactly the three
/// upstream cases: a bare `~`, a `~/`-prefixed path, and everything else untouched — `~user/` is
/// deliberately NOT expanded (see `crate::spawn::chain_graph::expand_home_path`, the crate's
/// other port of the same helper, for the upstream history).
fn expand_home(entry: &str) -> PathBuf {
    if entry == "~" {
        return crate::paths::home_dir();
    }
    if let Some(rest) = entry.strip_prefix("~/") {
        if rest.is_empty() {
            return crate::paths::home_dir();
        }
        return crate::paths::home_dir().join(rest);
    }
    PathBuf::from(entry)
}

/// pi `agentExclusionRoots` (`agents.ts:2416-2422`): every scope's `agentExcludeDirs` entries,
/// each trimmed, home-expanded and resolved against **the directory of the settings file that
/// declared it**, paired with its [`canonical_agent_path`].
///
/// `scopes` is `(settings file path, that file's `agentExcludeDirs`)` in upstream's order — user
/// first, then project (`[userSettingsPath, projectSettingsPath].flatMap(...)`). A scope with no
/// settings path or no entries contributes nothing; the flat concatenation is deliberate, since
/// exclusions from the two scopes UNION rather than one shadowing the other (unlike every scalar
/// `subagents.*` key, which is project-wins-outright).
#[must_use]
pub(crate) fn agent_exclusion_roots(scopes: &[(&Path, &[String])]) -> Vec<ExclusionRoot> {
    let mut roots = Vec::new();
    for (settings_path, entries) in scopes {
        let Some(base) = settings_path.parent() else {
            continue;
        };
        for entry in *entries {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            let resolved = resolve_against(base, &expand_home(trimmed));
            let real = canonicalize_resolved(&resolved);
            roots.push(ExclusionRoot { resolved, real });
        }
    }
    roots
}

/// pi `agentExclusions` (`agents.ts:2424-2431`), as a predicate over one candidate path.
///
/// Two arms, in upstream's order and with upstream's short-circuit: the cheap
/// `resolved`-prefix test over every root first, and only if that misses does the candidate get
/// canonicalised for the `real`-prefix test. The order matters for cost, not for outcome — but the
/// SECOND arm is what catches an agent reached through a symlink alias whose written path sits
/// nowhere near the excluded root.
#[must_use]
pub(crate) fn is_excluded(roots: &[ExclusionRoot], path: &Path) -> bool {
    if roots.is_empty() {
        return false;
    }
    if roots
        .iter()
        .any(|root| is_path_within(&root.resolved, path))
    {
        return true;
    }
    let real = canonical_agent_path(path);
    roots.iter().any(|root| is_path_within(&root.real, &real))
}

/// pi `expandAgentScanDirPattern` (`agents.ts:2433-2458`): expand one `agentScanDirs` entry into
/// the directories it names, which EXIST and are not excluded.
///
/// The wildcard rule is deliberately narrow and is pinned by a test: an entry may contain either
/// no `*` at all, or exactly ONE `*` which must be a WHOLE path segment (`parts[i] === "*"`).
/// Anything else — two wildcards, or a partial segment like `ag*nts` — yields NOTHING rather than
/// being interpreted (`agents.ts:2447`). Over-delivering here would turn a typo into a silent
/// widening of the discovery surface.
#[must_use]
pub(crate) fn expand_agent_scan_dir_pattern(
    pattern: &str,
    roots: &[ExclusionRoot],
) -> Vec<PathBuf> {
    let expanded = expand_home(pattern.trim());
    if expanded.as_os_str().is_empty() {
        return Vec::new();
    }
    let Some(text) = expanded.to_str() else {
        // A non-UTF-8 entry cannot contain `*` as a segment in any case upstream models; treat it
        // as the wildcard-free arm.
        let dir = resolve_against_cwd(&expanded);
        return if !is_excluded(roots, &dir) && dir.exists() {
            vec![dir]
        } else {
            Vec::new()
        };
    };
    let wildcards = text.matches('*').count();
    if wildcards == 0 {
        let dir = resolve_against_cwd(&expanded);
        return if !is_excluded(roots, &dir) && dir.exists() {
            vec![dir]
        } else {
            Vec::new()
        };
    }
    let parts: Vec<&str> = text.split('/').collect();
    let Some(wildcard_index) = parts.iter().position(|part| part.contains('*')) else {
        return Vec::new();
    };
    if wildcards != 1 || parts.get(wildcard_index).copied() != Some("*") {
        return Vec::new();
    }
    // `.get(..)`, not a slice index: this crate denies `clippy::indexing_slicing`, and the
    // bounds are only guaranteed by `position` having just returned this index.
    let head = parts.get(..wildcard_index).unwrap_or_default().join("/");
    let base = if head.is_empty() {
        PathBuf::from(std::path::MAIN_SEPARATOR_STR)
    } else {
        resolve_against_cwd(Path::new(&head))
    };
    if is_excluded(roots, &base) {
        return Vec::new();
    }
    let rest: &[&str] = parts.get(wildcard_index + 1..).unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect();
    // Upstream inherits `fs.readdirSync`'s order; sort so this port's discovery-priority
    // assignment over the expanded roots is deterministic rather than filesystem-dependent.
    children.sort();
    children
        .into_iter()
        .map(|child| rest.iter().fold(child, |acc, segment| acc.join(segment)))
        .filter(|dir| !is_excluded(roots, dir) && dir.exists())
        .collect()
}

/// pi `settingsAgentScanDirs` (`agents.ts:2460-2470`): every entry expanded via
/// [`expand_agent_scan_dir_pattern`], de-duplicated, FIRST occurrence's position kept (upstream's
/// `Set` insertion order).
#[must_use]
pub(crate) fn settings_agent_scan_dirs(
    entries: &[String],
    roots: &[ExclusionRoot],
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        for dir in expand_agent_scan_dir_pattern(entry, roots) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}
