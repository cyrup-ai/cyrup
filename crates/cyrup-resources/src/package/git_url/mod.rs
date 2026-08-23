//! Git source-URL parsing + security validation — 1:1 port of Pi `utils/git.ts` (1-227).
//!
//! Turns a user-supplied source string (`https://host/u/r`, `git@host:u/r@ref`, `git:host/u/r`, …)
//! into a validated [`ParsedGitUrl`], rejecting path-traversal / null-byte / backslash injection via
//! [`has_unsafe_git_install_part`] (git.ts:84-102 — a **security** control).
//!
//! The `hosted-git-info` host-shorthand layer (git.ts:180-223, the *primary* resolution path in
//! Pi's `parseGitUrl`) lives in the private `hosted` submodule as a dependency-free, hand-rolled
//! host table: a 1:1 port of npm `hosted-git-info` v9 (the version Pi pins, `package.json:51`)
//! restricted to the data that Pi's `buildGitSource` reads — `domain` / `user` / `project` /
//! `committish` — for the five well-known hosts (github, bitbucket, gitlab, gist, sourcehut).
//! This resolves bare GitHub shorthands (`git:owner/repo`) and `#committish` fragments on known
//! hosts. The URL *template* generators (`ssh`/`https`/`browse`/...) are irrelevant to
//! `parseGitUrl` and are not ported.
//! The generic-URL parser (`parseGenericGitUrl`, git.ts:126-163) remains the fallback (git.ts:225)
//! for hosts not in the table, exactly as in Pi.

mod hosted;

use crate::package::source::{PackageSource, PinRef};
use hosted::hosted_git_info_from_url;

/// Parsed git URL information (↔ Pi `GitSource`, git.ts:6-19).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedGitUrl {
    /// Clone URL (always valid for `git clone`, without the ref suffix).
    pub repo: String,
    /// Git host domain (e.g. `github.com`).
    pub host: String,
    /// Repository path (e.g. `user/repo`), `.git`/leading-slash stripped.
    pub path: String,
    /// Git ref (branch, tag, commit) if specified.
    pub reff: Option<String>,
    /// True if a ref was specified (Pi: `pinned = Boolean(ref)`, git.ts:117).
    pub pinned: bool,
}

impl ParsedGitUrl {
    /// Map a parsed git URL into a [`PackageSource`].
    ///
    /// Pi treats *any* explicitly-specified ref as pinned (won't auto-update, git.ts:117). cyrup's
    /// richer [`PinRef`] distinguishes commit-hashes from named refs but keeps Pi's pin semantics:
    /// a hex-looking ref becomes [`PinRef::Commit`], any other named ref becomes [`PinRef::Tag`] —
    /// both are pinned (R-09-020) and resolved by name at checkout, matching Pi's behavior. A source
    /// with no ref tracks the default branch ([`PinRef::Default`], updatable).
    pub fn into_source(self) -> PackageSource {
        let reff = match self.reff {
            None => PinRef::Default,
            Some(r) if is_hex_commit(&r) => PinRef::Commit(r),
            Some(r) => PinRef::Tag(r),
        };
        PackageSource::Git {
            url: self.repo,
            reff,
        }
    }
}

/// True for a 7-40 char all-hex string (an abbreviated or full git commit id).
fn is_hex_commit(s: &str) -> bool {
    let len = s.len();
    (7..=40).contains(&len) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Split a trailing `@ref` off a git URL (↔ Pi `splitRef`, git.ts:21-74). Returns the repo URL
/// (without the ref) and the ref if one was found. On any ambiguity the whole input is returned as
/// the repo with no ref, exactly as Pi does.
fn split_ref(url: &str) -> (String, Option<String>) {
    // scp-like `git@host:path[@ref]` (Pi `^git@([^:]+):(.+)$`).
    if let Some((host, path_with_ref)) = match_scp(url) {
        return match path_with_ref.find('@') {
            None => (url.to_string(), None),
            Some(sep) => {
                let repo_path = &path_with_ref[..sep];
                let reff = &path_with_ref[sep + 1..];
                if repo_path.is_empty() || reff.is_empty() {
                    (url.to_string(), None)
                } else {
                    (format!("git@{host}:{repo_path}"), Some(reff.to_string()))
                }
            }
        };
    }

    // Explicit-protocol URL: take the ref from an `@` in the path (Pi parses via `new URL`).
    if url.contains("://") {
        let Some((scheme_authority, path)) = url_scheme_authority_path(url) else {
            return (url.to_string(), None);
        };
        let path_no_lead = path.trim_start_matches('/');
        return match path_no_lead.find('@') {
            None => (url.to_string(), None),
            Some(sep) => {
                let repo_path = &path_no_lead[..sep];
                let reff = &path_no_lead[sep + 1..];
                if repo_path.is_empty() || reff.is_empty() {
                    (url.to_string(), None)
                } else {
                    (
                        format!("{scheme_authority}/{repo_path}"),
                        Some(reff.to_string()),
                    )
                }
            }
        };
    }

    // Bare `host/path[@ref]` form.
    let Some(slash) = url.find('/') else {
        return (url.to_string(), None);
    };
    let host = &url[..slash];
    let path_with_ref = &url[slash + 1..];
    match path_with_ref.find('@') {
        None => (url.to_string(), None),
        Some(sep) => {
            let repo_path = &path_with_ref[..sep];
            let reff = &path_with_ref[sep + 1..];
            if repo_path.is_empty() || reff.is_empty() {
                (url.to_string(), None)
            } else {
                (format!("{host}/{repo_path}"), Some(reff.to_string()))
            }
        }
    }
}

/// Match `git@host:rest` (Pi `^git@([^:]+):(.+)$`): host is non-empty with no colon, rest non-empty.
fn match_scp(url: &str) -> Option<(&str, &str)> {
    let s = url.strip_prefix("git@")?;
    let colon = s.find(':')?;
    let host = &s[..colon];
    let rest = &s[colon + 1..];
    if host.is_empty() || rest.is_empty() {
        return None;
    }
    Some((host, rest))
}

/// Split a scheme URL into `scheme://authority` and the path (query/fragment removed), mirroring the
/// parts of `new URL()` that [`split_ref`] needs. Returns `None` if there is no `://`.
fn url_scheme_authority_path(url: &str) -> Option<(String, String)> {
    let idx = url.find("://")?;
    let after = &url[idx + 3..];
    let path_start = after.find('/').unwrap_or(after.len());
    let authority = &after[..path_start];
    let scheme_authority = format!("{}{}", &url[..idx + 3], authority);
    let rest = &after[path_start..];
    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    Some((scheme_authority, rest[..path_end].to_string()))
}

/// Extract `(hostname, path)` from an explicit-protocol URL (↔ `new URL().hostname` / `.pathname`):
/// userinfo and port are stripped from the authority; the path has its leading slashes removed.
fn parse_protocol_url(url: &str) -> Option<(String, String)> {
    let (_scheme_authority, _) = url_scheme_authority_path(url)?;
    let idx = url.find("://")?;
    let after = &url[idx + 3..];
    let path_start = after.find('/').unwrap_or(after.len());
    let authority = &after[..path_start];
    let rest = &after[path_start..];
    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    let path = rest[..path_end].trim_start_matches('/').to_string();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host_port.split(':').next().unwrap_or(host_port).to_string();
    Some((host, path))
}

/// Decode `%XX` percent-escapes (↔ JS `decodeURIComponent`). Returns `None` on a malformed escape or
/// non-UTF-8 result, exactly as `decodeURIComponent` throws (git.ts:76-82 `decodeForValidation`).
fn decode_uri_component(s: &str) -> Option<String> {
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut iter = s.bytes();
    while let Some(b) = iter.next() {
        if b == b'%' {
            // A `%` with fewer than two following hex digits is malformed (decodeURIComponent throws).
            let hi = (iter.next()? as char).to_digit(16)?;
            let lo = (iter.next()? as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

/// SECURITY: reject path-traversal / injection in a host or repo-path component
/// (1:1 with Pi `hasUnsafeGitInstallPart`, git.ts:84-102). A component is unsafe if — in either its
/// raw or percent-decoded form — it contains a NUL or backslash, starts with `/`, contains `/` when
/// slashes are disallowed, or has a `..` path segment. A malformed percent-escape is itself unsafe.
pub fn has_unsafe_git_install_part(value: &str, allow_slash: bool) -> bool {
    let Some(decoded) = decode_uri_component(value) else {
        return true;
    };
    for candidate in [value, decoded.as_str()] {
        if candidate.contains('\0') || candidate.contains('\\') || candidate.starts_with('/') {
            return true;
        }
        if !allow_slash && candidate.contains('/') {
            return true;
        }
        if candidate.split('/').any(|seg| seg == "..") {
            return true;
        }
    }
    false
}

/// Build + validate a [`ParsedGitUrl`] from its parts (↔ Pi `buildGitSource`, git.ts:104-124).
/// Returns `None` if the path is absolute, the host/path is empty, the path has fewer than two
/// segments, or either component fails [`has_unsafe_git_install_part`].
fn build_git_source(
    repo: String,
    host: String,
    path: &str,
    reff: Option<String>,
) -> Option<ParsedGitUrl> {
    if path.starts_with('/') {
        return None;
    }
    let normalized_path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_start_matches('/');
    if host.is_empty() || normalized_path.is_empty() || normalized_path.split('/').count() < 2 {
        return None;
    }
    if has_unsafe_git_install_part(&host, false)
        || has_unsafe_git_install_part(normalized_path, true)
    {
        return None;
    }
    let pinned = reff.is_some();
    Some(ParsedGitUrl {
        repo,
        host,
        path: normalized_path.to_string(),
        reff,
        pinned,
    })
}

/// Parse a non-shorthand git URL (↔ Pi `parseGenericGitUrl`, git.ts:126-163): scp-like, explicit
/// protocol, or bare `host/path` (the bare form must have a dotted host or `localhost`).
fn parse_generic_git_url(url: &str) -> Option<ParsedGitUrl> {
    let (repo_without_ref, reff) = split_ref(url);
    let mut repo = repo_without_ref.clone();
    let host;
    let path;

    if let Some((h, p)) = match_scp(&repo_without_ref) {
        host = h.to_string();
        path = p.to_string();
    } else if repo_without_ref.starts_with("https://")
        || repo_without_ref.starts_with("http://")
        || repo_without_ref.starts_with("ssh://")
        || repo_without_ref.starts_with("git://")
    {
        let (h, p) = parse_protocol_url(&repo_without_ref)?;
        host = h;
        path = p;
    } else {
        let slash = repo_without_ref.find('/')?;
        host = repo_without_ref[..slash].to_string();
        path = repo_without_ref[slash + 1..].to_string();
        if !host.contains('.') && host != "localhost" {
            return None;
        }
        repo = format!("https://{repo_without_ref}");
    }

    build_git_source(repo, host, &path, reff)
}

/// Parse a git source string into a validated [`ParsedGitUrl`] (↔ Pi `parseGitUrl`, git.ts:172-226).
///
/// Rules (git.ts:165-171): with a `git:` prefix, accept generic forms; without it, only explicit
/// `https`/`http`/`ssh`/`git` protocol URLs are accepted. Returns `None` for anything that is not a
/// git URL (so the caller can fall back to treating it as a local path, ↔ Pi `parseSource`).
pub fn parse_git_url(source: &str) -> Option<ParsedGitUrl> {
    let trimmed = source.trim();
    let has_git_prefix = trimmed.starts_with("git:");
    let url = if has_git_prefix {
        trimmed[4..].trim()
    } else {
        trimmed
    };

    if !has_git_prefix && !has_protocol_prefix(url) {
        return None;
    }

    let (split_repo, split_ref_opt) = split_ref(url);

    // hostedGitInfo resolution path (git.ts:181-205): try `${repo}#${ref}` (if a ref was split off)
    // then the raw url, expanding shorthands and pulling `#committish` off known hosts.
    if let Some(found) = resolve_hosted(&split_repo, &split_ref_opt, &split_repo, url) {
        return found;
    }

    // The `https://`-prefixed retry (git.ts:207-223): re-try the candidates with an https:// prefix
    // so a host-qualified shorthand still resolves; here `repo` is always `https://${split_repo}`.
    let https_repo = format!("https://{split_repo}");
    if let Some(found) = resolve_hosted(&https_repo, &split_ref_opt, &split_repo, url) {
        return found;
    }

    // Fallback to the generic parser for hosts outside the table (git.ts:225).
    parse_generic_git_url(url)
}

/// Run one of Pi's two `hostedGitInfo` candidate loops (git.ts:186-205 / 210-223). `repo_base` is
/// the repo string the loop builds its clone URL from (`split.repo` for the first loop, `https://…`
/// for the second); `prefixed` controls whether the https-prefix branch is forced (the second loop
/// always prefixes). Returns `Some(result)` once a candidate resolves; `None` to fall through.
fn resolve_hosted(
    repo_base: &str,
    split_ref_opt: &Option<String>,
    split_repo: &str,
    url: &str,
) -> Option<Option<ParsedGitUrl>> {
    let force_https = repo_base != split_repo;
    let mut candidates: Vec<String> = Vec::new();
    if let Some(r) = split_ref_opt {
        candidates.push(format!("{repo_base}#{r}"));
    }
    candidates.push(if force_https {
        format!("https://{url}")
    } else {
        url.to_string()
    });

    for candidate in &candidates {
        let Some(info) = hosted_git_info_from_url(candidate) else {
            continue;
        };
        // git.ts:189/213: a ref plus a `@` in the project means the shorthand swallowed the ref —
        // skip this candidate so the next one (or the generic parser) can try.
        if split_ref_opt.is_some() && info.project.contains('@') {
            continue;
        }
        let use_https_prefix = !split_repo.starts_with("http://")
            && !split_repo.starts_with("https://")
            && !split_repo.starts_with("ssh://")
            && !split_repo.starts_with("git://")
            && !split_repo.starts_with("git@");
        let repo = if force_https || use_https_prefix {
            format!("https://{split_repo}")
        } else {
            split_repo.to_string()
        };
        let path = format!(
            "{}/{}",
            info.user.as_deref().unwrap_or("null"),
            info.project
        );
        // git.ts:202/220: `info.committish || split.ref || undefined` — empty committish is falsy.
        let reff = info
            .committish
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| split_ref_opt.clone());
        return Some(build_git_source(repo, info.domain, &path, reff));
    }
    None
}

/// Case-insensitive test for a leading `https://` / `http://` / `ssh://` / `git://`
/// (↔ Pi `/^(https?|ssh|git):\/\//i`).
fn has_protocol_prefix(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("ssh://")
        || lower.starts_with("git://")
}
