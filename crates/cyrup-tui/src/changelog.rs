//! `/changelog` and the "What's New" startup notice — a port of pi's
//! `packages/coding-agent/src/utils/changelog.ts` @v1.1.0 (196 lines, read in full) and its three
//! consumers in `modes/interactive/interactive-mode.ts`: `handleChangelogCommand` (`:6774-6793`),
//! `getChangelogForDisplay` (`:1332-1356`) and the notice render (`:849-870`).
//!
//! # `[CYRUP-DELTA]` the changelog is EMBEDDED, not read from disk
//!
//! pi reads `getChangelogPath()` = `resolve(join(getPackageDir(), "CHANGELOG.md"))`
//! (`utils/config.ts:459-461`) at runtime, and ships `CHANGELOG.md` inside its npm package. cyrup
//! has no equivalent delivery: `cyrup_config::asset_dir()` is `CYRUP_ASSET_DIR`, else the exe dir
//! IF it holds `README.md`, else the nearest ancestor with a `Cargo.toml` — for a
//! `~/.cargo/bin/cyrup` installed by `cargo install` none of the three hit, and there is no
//! `include` rule, build script or release workflow that would put a file beside the binary. A
//! runtime read would therefore ALWAYS miss and the feature would be dead on every installed copy,
//! which is what held `TUI-011` open.
//!
//! So [`EMBEDDED`] is `include_str!` of the repo's own `CHANGELOG.md`: it cannot be absent, it
//! needs no packaging story, and it is the one option that survives `cargo install`. The cost is
//! that a changelog edit needs a rebuild to show up, which for release notes compiled into a
//! release binary is the correct coupling rather than a limitation.
//!
//! Everything below the delivery seam takes the markdown as a `&str`, so the parser and the gate
//! are exercised against fixtures rather than against whatever this repo's own file happens to say.

/// One `## x.y.z` section: the parsed version plus the section's text INCLUDING its header line,
/// which is pi's `currentLines = [line]` (`changelog.ts:139`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogEntry {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub content: String,
}

impl ChangelogEntry {
    /// `entryVersion` (`changelog.ts:17-19`).
    pub fn version(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    /// `normalizeTag` (`changelog.ts:21-24`) for an entry: `v`-prefixed unless already prefixed.
    fn tag(&self) -> String {
        format!("v{}", self.version())
    }

    /// `compareVersions` (`changelog.ts:160-164`) as an `Ord`-shaped comparison.
    fn cmp_parts(&self, other: &(u64, u64, u64)) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch).cmp(other)
    }
}

/// The repo's own release notes, compiled in — see the module's `[CYRUP-DELTA]`.
pub const EMBEDDED: &str = include_str!("../../../CHANGELOG.md");

/// pi's `GITHUB_REPO` (`changelog.ts:11`), rebranded. Link rewriting targets cyrup's own repo.
const GITHUB_REPO: &str = "cyrup-ai/cyrup";
/// pi's `CHANGELOG_LINK_BASE_PATH` (`:12`) is `packages/coding-agent`, the directory its changelog
/// lives in, so a relative link resolves against it. cyrup's `CHANGELOG.md` is at the repo root, so
/// the base is empty and a relative target resolves against the root directly.
const LINK_BASE_PATH: &str = "";

/// `URL_SCHEME_RE` (`:14`): `^[a-z][a-z0-9+.-]*:` case-insensitively.
fn has_url_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for c in chars {
        if c == ':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-') {
            return false;
        }
    }
    false
}

/// `splitLocalTarget` (`:26-40`): `(fragment, path_part, query)`, fragment first because the `#`
/// is found before the `?` — a `?` AFTER a `#` belongs to the fragment, not the query.
fn split_local_target(target: &str) -> (&str, &str, &str) {
    let (before_hash, fragment) = match target.find('#') {
        Some(i) => target.split_at(i),
        None => (target, ""),
    };
    match before_hash.find('?') {
        Some(i) => {
            let (p, q) = before_hash.split_at(i);
            (fragment, p, q)
        }
        None => (fragment, before_hash, ""),
    }
}

/// POSIX `path.normalize` over a `/`-separated relative path: resolves `.` and `..` textually,
/// which is what `path.posix.normalize` does (it never touches the filesystem).
fn posix_normalize(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if matches!(out.last(), Some(&"..")) || out.is_empty() {
                    out.push("..");
                } else {
                    out.pop();
                }
            }
            p => out.push(p),
        }
    }
    if out.is_empty() {
        return ".".to_string();
    }
    out.join("/")
}

/// `resolveRepositoryPath` (`:46-58`): the repo-relative path a local link points at, or `None`
/// when it escapes the repo. `normalizePathPart` (`:42-44`) turns `\` into `/` first.
fn resolve_repository_path(target_path: &str) -> Option<String> {
    let normalized = target_path.replace('\\', "/");
    let joined = if let Some(stripped) = normalized.strip_prefix('/') {
        posix_normalize(stripped.trim_start_matches('/'))
    } else if LINK_BASE_PATH.is_empty() {
        posix_normalize(&normalized)
    } else {
        posix_normalize(&format!("{LINK_BASE_PATH}/{normalized}"))
    };
    if joined == "." || joined == ".." || joined.starts_with("../") {
        return None;
    }
    Some(joined)
}

/// `isDirectoryTarget` (`:60-67`): a trailing `/`, or a basename with no `.` in it.
fn is_directory_target(original_path: &str, repository_path: &str) -> bool {
    if original_path.ends_with('/') {
        return true;
    }
    let basename = repository_path
        .rsplit('/')
        .next()
        .unwrap_or(repository_path);
    !basename.contains('.')
}

/// `encodeURI` over a path: percent-encode everything `encodeURI` leaves out. `encodeURI` keeps
/// alphanumerics and `;,/?:@&=+$-_.!~*'()#`, so only the rest is escaped.
fn encode_uri_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || b";,/?:@&=+$-_.!~*'()#".contains(&b)
            || b == b'['
            || b == b']';
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `normalizeChangelogLinkTarget` (`:69-97`) minus the legacy-repo rewrite, which is pi's own
/// history (`badlogic`/`earendil-works` `pi-mono`) and has no cyrup counterpart.
///
/// Floating `blob|tree` + `main|master` refs are re-pointed at the entry's own TAG, so a link in
/// an old entry keeps resolving to the tree that entry described rather than to today's `main`.
fn normalize_link_target(target: &str, tag: &str) -> String {
    let repo_url = format!("https://github.com/{GITHUB_REPO}");
    let mut canonical = target.to_string();
    for route in ["blob", "tree"] {
        for branch in ["main", "master"] {
            let prefix = format!("{repo_url}/{route}/{branch}/");
            if let Some(rest) = canonical.strip_prefix(&prefix) {
                canonical = format!("{repo_url}/{route}/{tag}/{rest}");
            }
        }
    }
    if canonical.starts_with('#') || canonical.starts_with("//") || has_url_scheme(&canonical) {
        return canonical;
    }
    let (fragment, path_part, query) = split_local_target(&canonical);
    if path_part.is_empty() {
        return canonical;
    }
    let Some(repository_path) = resolve_repository_path(path_part) else {
        return canonical;
    };
    let route = if is_directory_target(path_part, &repository_path) {
        "tree"
    } else {
        "blob"
    };
    format!(
        "https://github.com/{GITHUB_REPO}/{route}/{tag}/{}{query}{fragment}",
        encode_uri_path(&repository_path)
    )
}

/// `normalizeChangelogLinks` (`:99-104`): rewrite every inline markdown link target in `markdown`.
///
/// `INLINE_MARKDOWN_LINK_RE` (`:15`) is `(!?\[[^\]\n]+\]\()([^\s)]+)((?:\s+[^)]*)?\))` — the `!?`
/// takes images too, the label may not contain `]` or a newline, the target may not contain
/// whitespace or `)`, and the optional tail is a title. Hand-scanned rather than regex-matched so
/// the crate needs no regex dependency for one pattern.
pub fn normalize_changelog_links(markdown: &str, entry: &ChangelogEntry) -> String {
    let tag = entry.tag();
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    // Every offset below comes from `find`ing an ASCII byte, or from arithmetic that steps over
    // one, so each is a char boundary and `split_at` cannot split a multi-byte character. The
    // workspace forbids `&s[a..b]` outright (`clippy::string_slice`), which is why this reads as a
    // chain of splits rather than slices.
    while let Some(open) = rest.find('[') {
        let (before_bracket, from_bracket) = rest.split_at(open);
        // `!?` — an image link's `!` is part of the matched prefix, so back up over it.
        let (lead, opener_extra) = if before_bracket.ends_with('!') {
            (
                before_bracket.strip_suffix('!').unwrap_or(before_bracket),
                1,
            )
        } else {
            (before_bracket, 0)
        };
        out.push_str(lead);
        let (opener, body) = from_bracket.split_at(1);
        // On any mismatch, emit what we consumed of the opener and resume after it, so a stray `[`
        // is never dropped.
        let emit_and_skip = |out: &mut String| {
            if opener_extra == 1 {
                out.push('!');
            }
            out.push_str(opener);
        };
        // `[^\]\n]+` then `](`
        let Some(close) = body
            .find(']')
            .filter(|&n| n > 0 && !body.split_at(n).0.contains('\n'))
        else {
            emit_and_skip(&mut out);
            rest = body;
            continue;
        };
        let (label_with_close, after_label) = body.split_at(close + 1);
        let Some(paren_body) = after_label.strip_prefix('(') else {
            emit_and_skip(&mut out);
            rest = body;
            continue;
        };
        // `[^\s)]+` — the target.
        let target_len = paren_body
            .find(|c: char| c.is_whitespace() || c == ')')
            .unwrap_or(paren_body.len());
        if target_len == 0 {
            emit_and_skip(&mut out);
            rest = body;
            continue;
        }
        let (target, after_target) = paren_body.split_at(target_len);
        // `(?:\s+[^)]*)?\)` — an optional whitespace-led title, then the close.
        let Some(paren) = after_target.find(')') else {
            emit_and_skip(&mut out);
            rest = body;
            continue;
        };
        let (tail, closing) = after_target.split_at(paren);
        if !tail.is_empty() && !tail.starts_with(char::is_whitespace) {
            emit_and_skip(&mut out);
            rest = body;
            continue;
        }
        if opener_extra == 1 {
            out.push('!');
        }
        out.push_str(opener);
        out.push_str(label_with_close);
        out.push('(');
        out.push_str(&normalize_link_target(target, &tag));
        out.push_str(tail);
        out.push(')');
        rest = closing.strip_prefix(')').unwrap_or(closing);
    }
    out.push_str(rest);
    out
}

/// `parseChangelog` (`changelog.ts:106-157`) over already-loaded markdown.
///
/// The version pattern is `/##\s+\[?(\d+)\.(\d+)\.(\d+)\]?/` matched against the WHOLE line
/// (`:134`), i.e. ANCHORED at the `## ` — so `## Version 1.2.3` and `## Unreleased (since 1.4.0)`
/// do NOT match, and upstream DROPS those sections (`:145-148` resets `currentVersion` to null and
/// clears the buffer). Getting that wrong turns prose headings into phantom releases, so the
/// anchoring is reproduced exactly rather than searching the line for a version anywhere in it.
///
/// Entries come back in FILE order, which a conventional changelog writes newest-first.
pub fn parse_changelog(markdown: &str) -> Vec<ChangelogEntry> {
    let mut entries: Vec<ChangelogEntry> = Vec::new();
    let mut current: Option<(u64, u64, u64)> = None;
    let mut lines: Vec<&str> = Vec::new();
    let flush = |entries: &mut Vec<ChangelogEntry>,
                 current: &Option<(u64, u64, u64)>,
                 lines: &Vec<&str>| {
        if let Some((major, minor, patch)) = *current
            && !lines.is_empty()
        {
            entries.push(ChangelogEntry {
                major,
                minor,
                patch,
                content: lines.join("\n").trim().to_string(),
            });
        }
    };
    for line in markdown.split('\n') {
        if let Some(rest) = line.strip_prefix("## ") {
            flush(&mut entries, &current, &lines);
            match parse_anchored_version(rest) {
                Some(v) => {
                    current = Some(v);
                    lines = vec![line];
                }
                None => {
                    current = None;
                    lines = Vec::new();
                }
            }
        } else if current.is_some() {
            lines.push(line);
        }
    }
    flush(&mut entries, &current, &lines);
    entries
}

/// The tail of `##\s+\[?(\d+)\.(\d+)\.(\d+)\]?` after `## `: leading whitespace (the `\s+` is
/// greedy past the one space `strip_prefix` took), an optional `[`, then three dot-separated runs
/// of digits. Anything else is not a version header.
fn parse_anchored_version(rest: &str) -> Option<(u64, u64, u64)> {
    let s = rest
        .trim_start()
        .strip_prefix('[')
        .unwrap_or_else(|| rest.trim_start());
    let mut it = s.split('.');
    let major = leading_digits(it.next()?)?;
    let minor = leading_digits(it.next()?)?;
    let patch = leading_digits(it.next()?)?;
    Some((major, minor, patch))
}

/// The leading run of digits of `s`, or `None` when it does not start with one — the regex's
/// `(\d+)` groups, which match a PREFIX of their segment (so `3]` and `3] - 2026-01-01` both give
/// `3`).
fn leading_digits(s: &str) -> Option<u64> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// `getNewEntries` (`changelog.ts:166-180`): the entries strictly newer than `last_version`.
///
/// `last_version` is split on `.` and each part run through `Number(...) || 0`, so a missing or
/// unparseable part degrades to `0` rather than failing — `"abc"` and `""` both read as `0.0.0`,
/// which makes every entry newer. Reproduced exactly: this is the arm a corrupt setting takes.
pub fn new_entries(entries: &[ChangelogEntry], last_version: &str) -> Vec<ChangelogEntry> {
    let mut parts = last_version.split('.');
    let last = (
        parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
        parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
        parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
    );
    entries
        .iter()
        .filter(|e| e.cmp_parts(&last) == std::cmp::Ordering::Greater)
        .cloned()
        .collect()
}

/// `handleChangelogCommand`'s body text (`interactive-mode.ts:6778-6785`): every entry, REVERSED
/// out of file order, links normalized, joined by a blank line — or pi's exact empty string.
///
/// The reversal is upstream's and is NOT shared with the startup notice, which joins
/// `newEntries` in file order (`:1353`). Both are reproduced as they are rather than unified.
pub fn command_markdown(markdown: &str) -> String {
    let entries = parse_changelog(markdown);
    if entries.is_empty() {
        return "No changelog entries found.".to_string();
    }
    entries
        .iter()
        .rev()
        .map(|e| normalize_changelog_links(&e.content, e))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What `getChangelogForDisplay` (`:1332-1356`) decides, as a value rather than a side effect so
/// the caller owns the persist and the test can read the decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupNotice {
    /// Show nothing and persist nothing — a resumed session (`:1333-1335`).
    Resumed,
    /// Show nothing but RECORD the version: pi's fresh install, which deliberately swallows the
    /// first changelog so a new user is not greeted by release notes (`:1342-1347`).
    RecordOnly,
    /// Show this markdown and persist the current version (`:1350-1354`).
    Show(String),
    /// Nothing new since the persisted version (`:1356`).
    UpToDate,
}

/// `getChangelogForDisplay` (`:1332-1356`), with the session's message count and the persisted
/// version passed in.
pub fn startup_notice(
    markdown: &str,
    has_messages: bool,
    last_version: Option<&str>,
) -> StartupNotice {
    if has_messages {
        return StartupNotice::Resumed;
    }
    let entries = parse_changelog(markdown);
    let Some(last) = last_version else {
        return StartupNotice::RecordOnly;
    };
    let fresh = new_entries(&entries, last);
    if fresh.is_empty() {
        return StartupNotice::UpToDate;
    }
    StartupNotice::Show(
        fresh
            .iter()
            .map(|e| normalize_changelog_links(&e.content, e))
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

/// The condensed notice's text under `collapseChangelog` (`interactive-mode.ts:858-860`):
/// `Updated to v{latest}. Use /changelog to view full changelog.`, where `{latest}` is the first
/// `## x.y.z` found in the notice markdown and falls back to the running version.
///
/// Upstream bolds the `/changelog` (`theme.bold`); the caller styles that span, so this returns
/// the three pieces rather than one string.
pub fn condensed_notice(notice_markdown: &str, running_version: &str) -> (String, String, String) {
    let latest = parse_changelog(notice_markdown)
        .first()
        .map(ChangelogEntry::version)
        .unwrap_or_else(|| running_version.to_string());
    (
        format!("Updated to v{latest}. Use "),
        "/changelog".to_string(),
        " to view full changelog.".to_string(),
    )
}
