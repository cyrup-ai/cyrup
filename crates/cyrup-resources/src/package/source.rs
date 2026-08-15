//! Package source + pin model (arch-09 §3.6, §6.4, R-09-020/021).

use std::path::PathBuf;

use cyrup_core::PackageId;

use crate::error::ResourceError;
use crate::package::git_url::parse_git_url;

/// Where a package is fetched from. Git is the primary channel; local path for dev; OCI deferred.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PackageSource {
    /// Primary channel (§7.6).
    Git {
        url: String,
        #[serde(default)]
        reff: PinRef,
    },
    /// Local dev install (no copy; referenced in place).
    Path { path: PathBuf },
    /// Deferred (R-09-021 candidate).
    Oci { reference: String },
}

/// Which git ref a package tracks. Tag/Commit are *pinned* and skipped by bulk update (R-09-020).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "ref", content = "value")]
pub enum PinRef {
    /// Default-branch HEAD — eligible for bulk update.
    #[default]
    Default,
    /// Tracked branch — eligible for bulk update.
    Branch(String),
    /// PINNED — skipped by bulk update (R-09-020).
    Tag(String),
    /// PINNED.
    Commit(String),
}

impl PinRef {
    /// True for Tag/Commit (R-09-020): bulk `update(All)` skips these.
    pub fn is_pinned(&self) -> bool {
        matches!(self, PinRef::Tag(_) | PinRef::Commit(_))
    }

    /// The git ref name to resolve/checkout, if any (branch/tag/commit).
    pub fn ref_name(&self) -> Option<&str> {
        match self {
            PinRef::Default => None,
            PinRef::Branch(s) | PinRef::Tag(s) | PinRef::Commit(s) => Some(s),
        }
    }
}

/// True if `value` is a local path rather than a package source / remote URL (1:1 with Pi
/// `isLocalPath`, utils/paths.ts:41-55): `npm:`/`git:`/`github:`/`http:`/`https:`/`ssh:` prefixes are
/// non-local; bare names, relative paths, and `file:` URLs are local.
pub fn is_local_path(value: &str) -> bool {
    let trimmed = value.trim();
    !(trimmed.starts_with("npm:")
        || trimmed.starts_with("git:")
        || trimmed.starts_with("github:")
        || trimmed.starts_with("http:")
        || trimmed.starts_with("https:")
        || trimmed.starts_with("ssh:"))
}

impl PackageSource {
    /// Parse a user-supplied source string into a validated [`PackageSource`] (1:1 with Pi
    /// `parseSource`, package-manager.ts:1399-1423, minus the dropped npm channel R-09-021).
    ///
    /// Routing matches Pi: an `npm:` spec is unsupported (`Err(Unsupported)`); a [`is_local_path`]
    /// string is a [`PackageSource::Path`]; otherwise the string is parsed as a git URL via
    /// [`parse_git_url`] (which applies the `hasUnsafeGitInstallPart` security validator); anything
    /// that is neither falls back to a local [`PackageSource::Path`].
    pub fn parse(source: &str) -> Result<PackageSource, ResourceError> {
        let trimmed = source.trim();
        if trimmed.starts_with("npm:") {
            // npm channel dropped in the Rust port (R-09-021): no JS runtime.
            return Err(ResourceError::UnsupportedNpm);
        }
        if is_local_path(trimmed) {
            return Ok(PackageSource::Path {
                path: PathBuf::from(trimmed),
            });
        }
        if let Some(parsed) = parse_git_url(trimmed) {
            return Ok(parsed.into_source());
        }
        Ok(PackageSource::Path {
            path: PathBuf::from(trimmed),
        })
    }

    /// The pin ref, if this source carries one (only Git does).
    pub fn pin(&self) -> PinRef {
        match self {
            PackageSource::Git { reff, .. } => reff.clone(),
            _ => PinRef::Default,
        }
    }

    /// Stable identity of an install (used to upsert the registry record).
    pub fn package_id(&self) -> PackageId {
        match self {
            PackageSource::Git { url, .. } => {
                PackageId::from(format!("git:{}", normalize_git(url)))
            }
            PackageSource::Path { path } => {
                let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                PackageId::from(format!("path:{}", abs.display()))
            }
            PackageSource::Oci { reference } => PackageId::from(format!("oci:{reference}")),
        }
    }
}

/// A unique identity for a package source, IGNORING version/ref — Pi `getPackageIdentity`
/// (`package-manager.ts:1676-1690` @v0.83.0), reached from `dedupePackages` (`:1696-1716`) and
/// `findAutoloadDeltaBase` (`:1301-1313`).
///
/// The three arms are pi's, in `parseSource` order (`:1435-1459`):
///
/// - `npm:<spec>` → `npm:<name>`, so `npm:x@1` and `npm:x@2` collide;
/// - a git URL → `git:<host>/<path>`, so the SSH and HTTPS URLs for one repo collide;
/// - anything else → `local:<resolved>`, where the path is resolved against `base_dir` — pi's
///   `getBaseDirForScope` (`:2071-2080`): `<cwd>/.cyrup` for a project entry, the agent dir for a
///   user entry. This is the arm that makes `"packages": ["./pack"]` mean two DIFFERENT packages
///   when both scopes declare it, which is what a raw source-string key gets wrong (CFG-026).
///
/// Not `PackageSource::parse` + `package_id`: that pair canonicalizes a local path through the
/// filesystem (`std::fs::canonicalize`, so a not-yet-installed path falls back to the raw relative
/// string) and rejects `npm:` outright, neither of which can key a dedupe.
pub fn package_identity(source: &str, base_dir: &std::path::Path) -> String {
    let trimmed = source.trim();
    if let Some(spec) = trimmed.strip_prefix("npm:") {
        return format!("npm:{}", npm_spec_name(spec.trim()));
    }
    if !is_local_path(trimmed)
        && let Some(parsed) = parse_git_url(trimmed)
    {
        return format!("git:{}/{}", parsed.host, parsed.path);
    }
    // `parseSource`'s final `return { type: "local", path: source }` (:1458) catches both a
    // [`is_local_path`] string and a non-local one `parse_git_url` could not read.
    format!(
        "local:{}",
        cyrup_config::paths::resolve_path_from_base(trimmed, base_dir).display()
    )
}

/// The name half of an npm spec — Pi `parseNpmSpec`'s `match[1]`
/// (`package-manager.ts:1719-1726` @v0.83.0, regex `^(@?[^@]+(?:\/[^@]+)?)(?:@(.+))?$`).
///
/// `x@1.0.0` → `x`, `@scope/pkg@1.0.0` → `@scope/pkg`, and a spec the regex cannot match at all
/// (`""`, `"@"`, `"@@x"`) is returned verbatim, exactly like upstream's `if (!match) return { name:
/// spec }` (`:1721-1723`).
fn npm_spec_name(spec: &str) -> &str {
    let (leading, rest) = match spec.strip_prefix('@') {
        Some(rest) => (1usize, rest),
        None => (0usize, spec),
    };
    // `[^@]+` needs at least one non-`@` character after the optional leading `@`.
    if rest.is_empty() || rest.starts_with('@') {
        return spec;
    }
    match rest.find('@') {
        Some(at) => &spec[..leading + at],
        None => spec,
    }
}

/// Normalize a git URL into a stable `host/user/repo` form (strip scheme, `.git`, trailing `/`).
fn normalize_git(url: &str) -> String {
    let mut s = url.trim();
    for prefix in ["https://", "http://", "ssh://", "git://", "file://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    // scp-like `git@host:user/repo`
    let s = s.strip_prefix("git@").unwrap_or(s);
    let s = s.replace(':', "/");
    let s = s.trim_end_matches('/');
    s.strip_suffix(".git").unwrap_or(s).to_string()
}

/// A filesystem-safe directory name derived from a [`PackageId`].
pub fn id_dir_name(id: &PackageId) -> String {
    id.as_str()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use std::path::Path;

    const PROJECT_BASE: &str = "/proj/.cyrup";
    const GLOBAL_BASE: &str = "/home/u/.cyrup/agent";

    /// Pi `getPackageIdentity`'s npm arm (`package-manager.ts:1678-1680` @v0.83.0) drops the
    /// version, so two entries pinning different versions of one package are ONE package.
    #[test]
    fn npm_identity_ignores_the_version_and_keeps_the_scope() {
        let base = Path::new(PROJECT_BASE);
        assert_eq!(package_identity("npm:x@1", base), "npm:x");
        assert_eq!(package_identity("npm:x@2", base), "npm:x");
        assert_eq!(package_identity("npm:x", base), "npm:x");
        assert_eq!(
            package_identity("npm:@scope/pkg@1.0.0", base),
            "npm:@scope/pkg"
        );
        assert_eq!(package_identity("npm:@scope/pkg", base), "npm:@scope/pkg");
        // `parseNpmSpec`'s `if (!match) return { name: spec }` (:1721-1723).
        assert_eq!(package_identity("npm:", base), "npm:");
        assert_eq!(package_identity("npm:@", base), "npm:@");
    }

    /// The git arm (`:1681-1684`) is `git:<host>/<path>`, which is exactly why an SSH URL and an
    /// HTTPS URL for one repository are the same package.
    #[test]
    fn git_identity_normalizes_ssh_and_https_and_drops_the_ref() {
        let base = Path::new(PROJECT_BASE);
        let https = package_identity("https://github.com/acme/pack.git", base);
        assert_eq!(https, "git:github.com/acme/pack");
        assert_eq!(package_identity("git:git@github.com:acme/pack", base), https);
        assert_eq!(
            package_identity("ssh://git@github.com/acme/pack.git", base),
            https
        );
        // The ref is part of neither half of the identity (`getPackageIdentity`'s doc comment,
        // ":1669-1674" — "ignoring version/ref"), so a branch pin does not fork the package.
        assert_eq!(
            package_identity("https://github.com/acme/pack.git#v2", base),
            https
        );
        // Scope-independent, unlike the local arm.
        assert_eq!(
            package_identity("https://github.com/acme/pack.git", Path::new(GLOBAL_BASE)),
            https
        );
    }

    /// The local arm (`:1685-1688`) resolves against the SCOPE base, so one source string is two
    /// identities — the defect CFG-026 records.
    #[test]
    fn a_relative_local_source_has_a_different_identity_per_scope() {
        let project = package_identity("./pack", Path::new(PROJECT_BASE));
        let global = package_identity("./pack", Path::new(GLOBAL_BASE));
        assert_eq!(project, "local:/proj/.cyrup/pack");
        assert_eq!(global, "local:/home/u/.cyrup/agent/pack");
        assert_ne!(project, global);
    }

    /// …while an ABSOLUTE (or `~`-anchored, or `file://`) local source is scope-independent, so the
    /// same tree declared in both scopes still dedupes to one entry.
    #[test]
    fn an_absolute_local_source_has_one_identity_across_scopes() {
        let project = package_identity("/abs/pack", Path::new(PROJECT_BASE));
        assert_eq!(project, "local:/abs/pack");
        assert_eq!(project, package_identity("/abs/pack", Path::new(GLOBAL_BASE)));
        // `resolvePath` normalizes first, so these three spellings are one identity.
        assert_eq!(project, package_identity("  /abs/pack  ", Path::new(GLOBAL_BASE)));
        assert_eq!(project, package_identity("file:///abs/pack", Path::new(GLOBAL_BASE)));
        assert_eq!(project, package_identity("/abs/./sub/../pack", Path::new(GLOBAL_BASE)));
    }

    /// `parseSource`'s final fallback (`:1458`): a non-local string `parseGitUrl` cannot read is a
    /// LOCAL path, not an error and not a distinct kind.
    #[test]
    fn an_unparseable_non_local_source_falls_back_to_the_local_arm() {
        let id = package_identity("http:", Path::new(PROJECT_BASE));
        assert!(
            id.starts_with("local:"),
            "expected the local fallback, got {id}"
        );
    }
}
