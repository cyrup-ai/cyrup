//! Git source-URL parsing + security validation — 1:1 port of Pi `utils/git.ts` (1-227).
//!
//! Turns a user-supplied source string (`https://host/u/r`, `git@host:u/r@ref`, `git:host/u/r`, …)
//! into a validated [`ParsedGitUrl`], rejecting path-traversal / null-byte / backslash injection via
//! [`has_unsafe_git_install_part`] (git.ts:84-102 — a **security** control).
//!
//! The `hosted-git-info` host-shorthand layer (git.ts:180-223, the *primary* resolution path in
//! Pi's `parseGitUrl`) is ported here as a dependency-free, hand-rolled host table: a 1:1 port of
//! npm `hosted-git-info` v9 (the version Pi pins, `package.json:51`) restricted to the data that
//! Pi's `buildGitSource` reads — `domain` / `user` / `project` / `committish` — for the five
//! well-known hosts (github, bitbucket, gitlab, gist, sourcehut). This resolves bare GitHub
//! shorthands (`git:owner/repo`) and `#committish` fragments on known hosts. The URL *template*
//! generators (`ssh`/`https`/`browse`/...) are irrelevant to `parseGitUrl` and are not ported.
//! The generic-URL parser (`parseGenericGitUrl`, git.ts:126-163) remains the fallback (git.ts:225)
//! for hosts not in the table, exactly as in Pi.

use crate::package::source::{PackageSource, PinRef};

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
        PackageSource::Git { url: self.repo, reff }
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
                    (format!("{scheme_authority}/{repo_path}"), Some(reff.to_string()))
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
    let normalized_path = path.strip_suffix(".git").unwrap_or(path).trim_start_matches('/');
    if host.is_empty()
        || normalized_path.is_empty()
        || normalized_path.split('/').count() < 2
    {
        return None;
    }
    if has_unsafe_git_install_part(&host, false)
        || has_unsafe_git_install_part(normalized_path, true)
    {
        return None;
    }
    let pinned = reff.is_some();
    Some(ParsedGitUrl { repo, host, path: normalized_path.to_string(), reff, pinned })
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
    let url = if has_git_prefix { trimmed[4..].trim() } else { trimmed };

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
    candidates.push(if force_https { format!("https://{url}") } else { url.to_string() });

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
        let path = format!("{}/{}", info.user.as_deref().unwrap_or("null"), info.project);
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

// ===========================================================================
// hosted-git-info port (npm `hosted-git-info` v9.0.3, the version Pi pins —
// coding-agent/package.json:51). Dependency-free, hand-rolled, restricted to the
// data Pi's `buildGitSource` reads: `domain` / `user` / `project` / `committish`,
// for the five well-known hosts (github, bitbucket, gitlab, gist, sourcehut).
// 1:1 port of `lib/{from-url,parse-url,hosts}.js`. Behavior verified against the
// real package output for the candidate strings Pi feeds `hostedGitInfo.fromUrl`.
// ===========================================================================

/// The subset of npm `hosted-git-info`'s `GitHost` that Pi's `buildGitSource` reads
/// (↔ `from-url.js` return + `GitHost` fields).
struct HostedInfo {
    /// Git host domain (e.g. `github.com`) — ↔ `info.domain`.
    domain: String,
    /// Owner/user (`None` only for gist host-less ids) — ↔ `info.user`.
    user: Option<String>,
    /// Repository/project slug — ↔ `info.project`.
    project: String,
    /// `#committish` fragment — `None` for shortcuts without one, `Some("")` for url
    /// forms without one (both falsy in Pi's `info.committish || split.ref`).
    committish: Option<String>,
}

/// One of the five well-known git hosts (↔ `hosts.js` `gitHosts`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum GitHostKind {
    Github,
    Bitbucket,
    Gitlab,
    Gist,
    Sourcehut,
}

/// The global protocol table (↔ `index.js` `GitHost.#protocols` plus the per-host
/// shortcut protocols added by `addHost`). Used by [`correct_protocol`] to decide
/// whether a leading `scheme:` is a recognized protocol.
const GLOBAL_PROTOCOLS: &[&str] = &[
    "git+ssh:",
    "ssh:",
    "git+https:",
    "git:",
    "http:",
    "https:",
    "git+http:",
    "github:",
    "bitbucket:",
    "gitlab:",
    "gist:",
    "sourcehut:",
];

/// Path segments extracted from a host URL (↔ the `{ user, project, committish }` a
/// host's `extract(url)` returns; raw, pre-`decodeURIComponent`).
struct Segments {
    user: Option<String>,
    project: String,
    committish: String,
}

impl GitHostKind {
    /// The host domain (↔ `host.domain`).
    fn domain(self) -> &'static str {
        match self {
            GitHostKind::Github => "github.com",
            GitHostKind::Bitbucket => "bitbucket.org",
            GitHostKind::Gitlab => "gitlab.com",
            GitHostKind::Gist => "gist.github.com",
            GitHostKind::Sourcehut => "git.sr.ht",
        }
    }

    /// The protocols this host accepts for url-form parsing (↔ `host.protocols`).
    fn protocols(self) -> &'static [&'static str] {
        match self {
            GitHostKind::Github => {
                &["git:", "http:", "git+ssh:", "git+https:", "ssh:", "https:"]
            }
            GitHostKind::Bitbucket | GitHostKind::Gitlab => {
                &["git+ssh:", "git+https:", "ssh:", "https:"]
            }
            GitHostKind::Gist => &["git:", "git+ssh:", "git+https:", "ssh:", "https:"],
            GitHostKind::Sourcehut => &["git+ssh:", "https:"],
        }
    }

    /// Resolve a `scheme:` shortcut protocol to a host (↔ `gitHosts.byShortcut`).
    fn from_shortcut(protocol: &str) -> Option<Self> {
        match protocol {
            "github:" => Some(GitHostKind::Github),
            "bitbucket:" => Some(GitHostKind::Bitbucket),
            "gitlab:" => Some(GitHostKind::Gitlab),
            "gist:" => Some(GitHostKind::Gist),
            "sourcehut:" => Some(GitHostKind::Sourcehut),
            _ => None,
        }
    }

    /// Resolve a hostname to a host (↔ `gitHosts.byDomain`).
    fn from_domain(domain: &str) -> Option<Self> {
        match domain {
            "github.com" => Some(GitHostKind::Github),
            "bitbucket.org" => Some(GitHostKind::Bitbucket),
            "gitlab.com" => Some(GitHostKind::Gitlab),
            "gist.github.com" => Some(GitHostKind::Gist),
            "git.sr.ht" => Some(GitHostKind::Sourcehut),
            _ => None,
        }
    }

    /// Extract `{ user, project, committish }` from a parsed host URL (↔ each host's
    /// `extract(url)` in `hosts.js`). `pathname` is the URL path; `hash` is `#frag`/``.
    fn extract(self, pathname: &str, hash: &str) -> Option<Segments> {
        let hash_committish = hash.strip_prefix('#').unwrap_or(hash).to_string();
        match self {
            GitHostKind::Github => {
                let parts = split_limit(pathname, 5);
                let user = parts.get(1).copied().unwrap_or("");
                let mut project = parts.get(2).copied().unwrap_or("").to_string();
                let type_ = parts.get(3).copied();
                // `if (type && type !== 'tree') return` — truthy non-`tree` rejects.
                if let Some(t) = type_
                    && !t.is_empty()
                    && t != "tree"
                {
                    return None;
                }
                // `if (!type) committish = url.hash.slice(1)` — falsy = missing or "".
                let committish = if type_.is_none_or(str::is_empty) {
                    hash_committish
                } else {
                    parts.get(4).copied().unwrap_or("").to_string()
                };
                if let Some(p) = project.strip_suffix(".git") {
                    project = p.to_string();
                }
                if user.is_empty() || project.is_empty() {
                    return None;
                }
                Some(Segments { user: Some(user.to_string()), project, committish })
            }
            GitHostKind::Bitbucket => {
                let parts = split_limit(pathname, 4);
                let user = parts.get(1).copied().unwrap_or("");
                let mut project = parts.get(2).copied().unwrap_or("").to_string();
                let aux = parts.get(3).copied().unwrap_or("");
                if aux == "get" {
                    return None;
                }
                if let Some(p) = project.strip_suffix(".git") {
                    project = p.to_string();
                }
                if user.is_empty() || project.is_empty() {
                    return None;
                }
                Some(Segments { user: Some(user.to_string()), project, committish: hash_committish })
            }
            GitHostKind::Gitlab => {
                let path = pathname.strip_prefix('/').unwrap_or(pathname);
                if path.contains("/-/") || path.contains("/archive.tar.gz") {
                    return None;
                }
                let mut segments: Vec<&str> = path.split('/').collect();
                let project_raw = segments.pop().unwrap_or("");
                let mut project = project_raw.to_string();
                if let Some(p) = project.strip_suffix(".git") {
                    project = p.to_string();
                }
                let user = segments.join("/");
                if user.is_empty() || project.is_empty() {
                    return None;
                }
                Some(Segments { user: Some(user), project, committish: hash_committish })
            }
            GitHostKind::Gist => {
                let parts = split_limit(pathname, 4);
                let user_p = parts.get(1).copied().unwrap_or("");
                let project_p = parts.get(2).copied();
                let aux = parts.get(3).copied().unwrap_or("");
                if aux == "raw" {
                    return None;
                }
                // `if (!project) { if (!user) return; project = user; user = null }`.
                let (user, mut project) = match project_p {
                    Some(p) if !p.is_empty() => (Some(user_p.to_string()), p.to_string()),
                    _ => {
                        if user_p.is_empty() {
                            return None;
                        }
                        (None, user_p.to_string())
                    }
                };
                if let Some(p) = project.strip_suffix(".git") {
                    project = p.to_string();
                }
                Some(Segments { user, project, committish: hash_committish })
            }
            GitHostKind::Sourcehut => {
                let parts = split_limit(pathname, 4);
                let user = parts.get(1).copied().unwrap_or("");
                let mut project = parts.get(2).copied().unwrap_or("").to_string();
                let aux = parts.get(3).copied().unwrap_or("");
                if aux == "archive" {
                    return None;
                }
                if let Some(p) = project.strip_suffix(".git") {
                    project = p.to_string();
                }
                if user.is_empty() || project.is_empty() {
                    return None;
                }
                Some(Segments { user: Some(user.to_string()), project, committish: hash_committish })
            }
        }
    }
}

/// `str.split(sep, limit)` semantics: split fully, then truncate to `n` parts
/// (↔ JS `url.pathname.split('/', N)`).
fn split_limit(s: &str, n: usize) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split('/').collect();
    parts.truncate(n);
    parts
}

/// `String.prototype.indexOf` as an `i64` with `-1` for "not found".
fn find_i(s: &str, ch: char) -> i64 {
    s.find(ch).map_or(-1, |i| i as i64)
}

/// `lastIndexOfBefore(str, char, beforeChar)` (↔ `parse-url.js`): the last index of
/// `ch` at or before the first occurrence of `before` (whole string if `before`
/// absent). `-1` when not found.
fn last_index_of_before(s: &str, ch: char, before: char) -> i64 {
    let limit = s.find(before).map_or(s.len(), |i| i);
    let hay = s.get(..(limit + 1).min(s.len())).unwrap_or(s);
    hay.rfind(ch).map_or(-1, |i| i as i64)
}

/// A minimal `new URL()` for the subset of forms Pi feeds `hostedGitInfo.fromUrl`.
/// Returns `None` where `new URL` would throw (so `safeUrl` yields `undefined`).
struct MiniUrl {
    /// `scheme:` (lower-cased) — ↔ `url.protocol`.
    protocol: String,
    /// Host (lower-cased, userinfo/port stripped) — ↔ `url.hostname`.
    hostname: String,
    /// Path (dot-segments normalized for authority forms) — ↔ `url.pathname`.
    pathname: String,
    /// `#fragment` including the leading `#`, or `""` — ↔ `url.hash`.
    hash: String,
}

/// Classify a path segment as a single-dot / double-dot / ordinary segment, treating
/// `%2e` (any case) as a literal `.` exactly as the WHATWG URL path-state machine does.
enum DotKind {
    Double,
    Single,
    None,
}

fn dot_kind(seg: &str) -> DotKind {
    let lowered = seg.to_ascii_lowercase().replace("%2e", ".");
    if lowered == ".." {
        DotKind::Double
    } else if lowered == "." {
        DotKind::Single
    } else {
        DotKind::None
    }
}

/// WHATWG dot-segment normalization of an authority-form path (↔ `new URL().pathname`
/// collapsing `..` / `.` and `%2e` for hosts with an authority component).
fn normalize_path(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    let last = segs.len().saturating_sub(1);
    let mut out: Vec<&str> = Vec::new();
    for (i, seg) in segs.iter().copied().enumerate() {
        if i == 0 {
            // leading empty segment = the root slash; re-added by the final join.
            continue;
        }
        match dot_kind(seg) {
            DotKind::Double => {
                out.pop();
                if i == last {
                    out.push("");
                }
            }
            DotKind::Single => {
                if i == last {
                    out.push("");
                }
            }
            DotKind::None => out.push(seg),
        }
    }
    format!("/{}", out.join("/"))
}

/// Parse a corrected URL string the way `new URL()` would for our subset. `None` mirrors
/// a `new URL` throw.
fn mini_url(s: &str) -> Option<MiniUrl> {
    let colon = s.find(':')?;
    let scheme = s.get(..colon)?;
    let mut sc = scheme.chars();
    let first = sc.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !sc.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
        return None;
    }
    let protocol = format!("{}:", scheme.to_ascii_lowercase());
    let rest = s.get(colon + 1..)?;

    let (pre_frag, hash) = match rest.find('#') {
        Some(h) => (rest.get(..h)?, rest.get(h..)?.to_string()),
        None => (rest, String::new()),
    };
    let pre = match pre_frag.find('?') {
        Some(q) => pre_frag.get(..q)?,
        None => pre_frag,
    };

    let (hostname, pathname) = if let Some(after) = pre.strip_prefix("//") {
        let path_start = after.find('/').unwrap_or(after.len());
        let authority = after.get(..path_start)?;
        let path = after.get(path_start..)?;
        let host_part = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let hostname = host_part.split(':').next().unwrap_or(host_part).to_ascii_lowercase();
        let pathname = if path.is_empty() { String::new() } else { normalize_path(path) };
        (hostname, pathname)
    } else {
        // Opaque path (e.g. `github:owner/repo`) — no authority, no normalization.
        (String::new(), pre.to_string())
    };

    Some(MiniUrl { protocol, hostname, pathname, hash })
}

/// `correctProtocol(arg, protocols)` (↔ `parse-url.js`): insert `//` after a bare
/// `scheme:`, or `git+ssh://` for scp-shaped `user@host:...`, leaving recognized
/// protocols untouched.
fn correct_protocol(arg: &str) -> String {
    let first_colon = arg.find(':');
    let proto = match first_colon {
        Some(i) => arg.get(..=i).unwrap_or(""),
        None => "",
    };
    if GLOBAL_PROTOCOLS.contains(&proto) {
        return arg.to_string();
    }
    if let Some(i) = first_colon
        && arg.get(i..).is_some_and(|s| s.starts_with("://"))
    {
        return arg.to_string();
    }
    let first_at = find_i(arg, '@');
    let fc = first_colon.map_or(-1, |i| i as i64);
    if first_at > -1 {
        return if first_at > fc { format!("git+ssh://{arg}") } else { arg.to_string() };
    }
    match first_colon {
        Some(i) => format!("{}//{}", arg.get(..=i).unwrap_or(""), arg.get(i + 1..).unwrap_or("")),
        None => format!("//{arg}"),
    }
}

/// `correctUrl(giturl)` (↔ `parse-url.js`): rewrite an scp-style URL into something
/// `new URL()` accepts (replace the host/path colon with `/`, prepend `git+ssh://`).
fn correct_url(giturl: &str) -> String {
    let first_at = last_index_of_before(giturl, '@', '#');
    let last_colon = last_index_of_before(giturl, ':', '#');
    let mut g = giturl.to_string();
    if last_colon > first_at
        && let Ok(i) = usize::try_from(last_colon)
    {
        let before = g.get(..i).unwrap_or("");
        let after = g.get(i + 1..).unwrap_or("");
        g = format!("{before}/{after}");
    }
    if last_index_of_before(&g, ':', '#') == -1 && !g.contains("//") {
        g = format!("git+ssh://{g}");
    }
    g
}

/// `parseUrl(giturl, protocols)` (↔ `parse-url.js`): `safeUrl(correctProtocol(..))`
/// then a fallback `safeUrl(correctUrl(..))`.
fn parse_url(giturl: &str) -> Option<MiniUrl> {
    let with_protocol = correct_protocol(giturl);
    if let Some(u) = mini_url(&with_protocol) {
        return Some(u);
    }
    mini_url(&correct_url(&with_protocol))
}

/// `isGitHubShorthand(arg)` (↔ `from-url.js`): a bare `owner/repo[#ref]` GitHub
/// shorthand (a single pre-`#` slash, no protocol/space/`@`/leading-dot before `#`).
fn is_github_shorthand(arg: &str) -> bool {
    let first_hash = find_i(arg, '#');
    let first_slash = find_i(arg, '/');
    let second_slash = {
        let start = (first_slash + 1).max(0) as usize;
        arg.get(start..).and_then(|s| s.find('/')).map_or(-1, |p| (p + start) as i64)
    };
    let first_colon = find_i(arg, ':');
    let first_space = arg.find(char::is_whitespace).map_or(-1, |i| i as i64);
    let first_at = find_i(arg, '@');

    let space_only_after_hash = first_space < 0 || (first_hash > -1 && first_space > first_hash);
    let at_only_after_hash = first_at == -1 || (first_hash > -1 && first_at > first_hash);
    let colon_only_after_hash = first_colon == -1 || (first_hash > -1 && first_colon > first_hash);
    let second_slash_only_after_hash =
        second_slash == -1 || (first_hash > -1 && second_slash > first_hash);
    let has_slash = first_slash > 0;
    let does_not_end_with_slash = if first_hash > -1 {
        usize::try_from(first_hash - 1).ok().and_then(|i| arg.as_bytes().get(i)) != Some(&b'/')
    } else {
        !arg.ends_with('/')
    };
    let does_not_start_with_dot = !arg.starts_with('.');

    space_only_after_hash
        && has_slash
        && does_not_end_with_slash
        && does_not_start_with_dot
        && at_only_after_hash
        && colon_only_after_hash
        && second_slash_only_after_hash
}

/// `hostedGitInfo.fromUrl(giturl)` restricted to the `domain`/`user`/`project`/
/// `committish` Pi reads (↔ `from-url.js` module export + `index.js` `GitHost`).
/// Returns `None` for inputs that resolve to no known host (Pi's `undefined`).
fn hosted_git_info_from_url(giturl: &str) -> Option<HostedInfo> {
    if giturl.is_empty() {
        return None;
    }
    let corrected =
        if is_github_shorthand(giturl) { format!("github:{giturl}") } else { giturl.to_string() };
    let parsed = parse_url(&corrected)?;

    let shortcut = GitHostKind::from_shortcut(&parsed.protocol);
    let domain_key = parsed.hostname.strip_prefix("www.").unwrap_or(&parsed.hostname);
    let host_kind = shortcut.or_else(|| GitHostKind::from_domain(domain_key))?;

    if shortcut.is_some() {
        // Shortcut branch (`gitHostShortcut`): the path is `user/project`, auth `@`
        // trimmed, committish from `#frag`.
        let raw = parsed.pathname.strip_prefix('/').unwrap_or(&parsed.pathname);
        let trimmed = match raw.find('@') {
            Some(at) => raw.get(at + 1..).unwrap_or(""),
            None => raw,
        };
        let (user, project_src) = match trimmed.rfind('/') {
            Some(ls) => {
                let user_raw = decode_uri_component(trimmed.get(..ls).unwrap_or(""))?;
                let user = if user_raw.is_empty() { None } else { Some(user_raw) };
                (user, trimmed.get(ls + 1..).unwrap_or(""))
            }
            None => (None, trimmed),
        };
        let mut project = decode_uri_component(project_src)?;
        if let Some(p) = project.strip_suffix(".git") {
            project = p.to_string();
        }
        let committish = if parsed.hash.is_empty() {
            None
        } else {
            Some(decode_uri_component(parsed.hash.strip_prefix('#').unwrap_or(&parsed.hash))?)
        };
        Some(HostedInfo { domain: host_kind.domain().to_string(), user, project, committish })
    } else {
        // Url branch: the host's `extract` plus a per-host protocol gate.
        if !host_kind.protocols().contains(&parsed.protocol.as_str()) {
            return None;
        }
        let seg = host_kind.extract(&parsed.pathname, &parsed.hash)?;
        // Pi: `user = segments.user && decodeURIComponent(segments.user)` — empty
        // short-circuits without decoding; `null` stays `null`.
        let user = match seg.user.as_deref() {
            Some("") => Some(String::new()),
            Some(u) => Some(decode_uri_component(u)?),
            None => None,
        };
        let project = decode_uri_component(&seg.project)?;
        let committish = decode_uri_component(&seg.committish)?;
        Some(HostedInfo {
            domain: host_kind.domain().to_string(),
            user,
            project,
            committish: Some(committish),
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod hosted_tests {
    use super::*;

    /// `hosted_git_info_from_url` matches the real npm `hosted-git-info@9.0.3` output
    /// for the candidate strings Pi feeds it (verified against the package).
    #[test]
    fn hosted_git_info_matches_reference() {
        let info = hosted_git_info_from_url("github:owner/repo").expect("shorthand resolves");
        assert_eq!(info.domain, "github.com");
        assert_eq!(info.user.as_deref(), Some("owner"));
        assert_eq!(info.project, "repo");
        assert_eq!(info.committish, None);

        let gl = hosted_git_info_from_url("gitlab:group/sub/proj").expect("gitlab shorthand");
        assert_eq!(gl.user.as_deref(), Some("group/sub"));
        assert_eq!(gl.project, "proj");

        let gist = hosted_git_info_from_url("gist:abc123").expect("gist shorthand");
        assert_eq!(gist.user, None);
        assert_eq!(gist.project, "abc123");

        let url = hosted_git_info_from_url("https://github.com/user/repo.git").expect("https url");
        assert_eq!(url.user.as_deref(), Some("user"));
        assert_eq!(url.project, "repo");
        assert_eq!(url.committish.as_deref(), Some(""));

        let scp = hosted_git_info_from_url("git@github.com:user/repo#v1.0").expect("scp");
        assert_eq!(scp.user.as_deref(), Some("user"));
        assert_eq!(scp.committish.as_deref(), Some("v1.0"));

        // `..` is normalized away by the URL path machine (so it is NOT a traversal).
        let dots = hosted_git_info_from_url("https://github.com/../etc/passwd").expect("normalized");
        assert_eq!(dots.user.as_deref(), Some("etc"));
        assert_eq!(dots.project, "passwd");

        // `%2e%2e` collapses to nothing, leaving a single-segment path → no host match.
        assert!(hosted_git_info_from_url("https://github.com/%2e%2e/secrets").is_none());

        // A non-host domain does not resolve.
        assert!(hosted_git_info_from_url("https://example.com/u/r").is_none());
    }
}
