//! hosted-git-info port (npm `hosted-git-info` v9.0.3, the version Pi pins —
//! coding-agent/package.json:51). Dependency-free, hand-rolled, restricted to the
//! data Pi's `buildGitSource` reads: `domain` / `user` / `project` / `committish`,
//! for the five well-known hosts (github, bitbucket, gitlab, gist, sourcehut).
//! 1:1 port of `lib/{from-url,parse-url,hosts}.js`. Behavior verified against the
//! real package output for the candidate strings Pi feeds `hostedGitInfo.fromUrl`.

use super::decode_uri_component;

/// The subset of npm `hosted-git-info`'s `GitHost` that Pi's `buildGitSource` reads
/// (↔ `from-url.js` return + `GitHost` fields).
pub(super) struct HostedInfo {
    /// Git host domain (e.g. `github.com`) — ↔ `info.domain`.
    pub(super) domain: String,
    /// Owner/user (`None` only for gist host-less ids) — ↔ `info.user`.
    pub(super) user: Option<String>,
    /// Repository/project slug — ↔ `info.project`.
    pub(super) project: String,
    /// `#committish` fragment — `None` for shortcuts without one, `Some("")` for url
    /// forms without one (both falsy in Pi's `info.committish || split.ref`).
    pub(super) committish: Option<String>,
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
            GitHostKind::Github => &["git:", "http:", "git+ssh:", "git+https:", "ssh:", "https:"],
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
                Some(Segments {
                    user: Some(user.to_string()),
                    project,
                    committish,
                })
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
                Some(Segments {
                    user: Some(user.to_string()),
                    project,
                    committish: hash_committish,
                })
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
                Some(Segments {
                    user: Some(user),
                    project,
                    committish: hash_committish,
                })
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
                Some(Segments {
                    user,
                    project,
                    committish: hash_committish,
                })
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
                Some(Segments {
                    user: Some(user.to_string()),
                    project,
                    committish: hash_committish,
                })
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
    let limit = s.find(before).unwrap_or(s.len());
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
        let hostname = host_part
            .split(':')
            .next()
            .unwrap_or(host_part)
            .to_ascii_lowercase();
        let pathname = if path.is_empty() {
            String::new()
        } else {
            normalize_path(path)
        };
        (hostname, pathname)
    } else {
        // Opaque path (e.g. `github:owner/repo`) — no authority, no normalization.
        (String::new(), pre.to_string())
    };

    Some(MiniUrl {
        protocol,
        hostname,
        pathname,
        hash,
    })
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
        return if first_at > fc {
            format!("git+ssh://{arg}")
        } else {
            arg.to_string()
        };
    }
    match first_colon {
        Some(i) => format!(
            "{}//{}",
            arg.get(..=i).unwrap_or(""),
            arg.get(i + 1..).unwrap_or("")
        ),
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
        arg.get(start..)
            .and_then(|s| s.find('/'))
            .map_or(-1, |p| (p + start) as i64)
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
        usize::try_from(first_hash - 1)
            .ok()
            .and_then(|i| arg.as_bytes().get(i))
            != Some(&b'/')
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
pub(super) fn hosted_git_info_from_url(giturl: &str) -> Option<HostedInfo> {
    if giturl.is_empty() {
        return None;
    }
    let corrected = if is_github_shorthand(giturl) {
        format!("github:{giturl}")
    } else {
        giturl.to_string()
    };
    let parsed = parse_url(&corrected)?;

    let shortcut = GitHostKind::from_shortcut(&parsed.protocol);
    let domain_key = parsed
        .hostname
        .strip_prefix("www.")
        .unwrap_or(&parsed.hostname);
    let host_kind = shortcut.or_else(|| GitHostKind::from_domain(domain_key))?;

    if shortcut.is_some() {
        // Shortcut branch (`gitHostShortcut`): the path is `user/project`, auth `@`
        // trimmed, committish from `#frag`.
        let raw = parsed
            .pathname
            .strip_prefix('/')
            .unwrap_or(&parsed.pathname);
        let trimmed = match raw.find('@') {
            Some(at) => raw.get(at + 1..).unwrap_or(""),
            None => raw,
        };
        let (user, project_src) = match trimmed.rfind('/') {
            Some(ls) => {
                let user_raw = decode_uri_component(trimmed.get(..ls).unwrap_or(""))?;
                let user = if user_raw.is_empty() {
                    None
                } else {
                    Some(user_raw)
                };
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
            Some(decode_uri_component(
                parsed.hash.strip_prefix('#').unwrap_or(&parsed.hash),
            )?)
        };
        Some(HostedInfo {
            domain: host_kind.domain().to_string(),
            user,
            project,
            committish,
        })
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
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
        let dots =
            hosted_git_info_from_url("https://github.com/../etc/passwd").expect("normalized");
        assert_eq!(dots.user.as_deref(), Some("etc"));
        assert_eq!(dots.project, "passwd");

        // `%2e%2e` collapses to nothing, leaving a single-segment path → no host match.
        assert!(hosted_git_info_from_url("https://github.com/%2e%2e/secrets").is_none());

        // A non-host domain does not resolve.
        assert!(hosted_git_info_from_url("https://example.com/u/r").is_none());
    }
}
