//! Git source-URL parsing, hosted shorthands, committish, traversal/injection rejection, and
//! `PackageSource` routing across git / local / npm (G2, CFG-052).

use crate::{PackageSource, ParsedGitUrl, PinRef, has_unsafe_git_install_part, parse_git_url};

// ===========================================================================
// G2 — git source-URL parsing + security validation (utils/git.ts)
// ===========================================================================

#[test]
fn git_url_parse_protocol_scp_and_shorthand() {
    // Explicit HTTPS URL → host/path extracted, `.git` stripped, no ref (git.ts:126-163).
    let p = parse_git_url("https://github.com/user/repo.git").expect("https url parses");
    assert_eq!(p.host, "github.com");
    assert_eq!(p.path, "user/repo");
    assert_eq!(p.repo, "https://github.com/user/repo.git");
    assert_eq!(p.reff, None);
    assert!(!p.pinned);

    // scp-like `git@host:path@ref` → ref split off, repo rebuilt without the ref (git.ts:21-36).
    let s = parse_git_url("git:git@github.com:user/repo@v1.0").expect("scp form parses via git:");
    assert_eq!(s.host, "github.com");
    assert_eq!(s.path, "user/repo");
    assert_eq!(s.repo, "git@github.com:user/repo");
    assert_eq!(s.reff.as_deref(), Some("v1.0"));
    assert!(s.pinned, "an explicit ref pins (git.ts:117)");

    // `git:` host-qualified shorthand resolves through the generic parser, prefixing https.
    let g = parse_git_url("git:github.com/user/repo").expect("git: host-qualified parses");
    assert_eq!(g.repo, "https://github.com/user/repo");
    assert_eq!(g.path, "user/repo");

    // Without a git: prefix, a bare host/path is NOT a git URL (no protocol) — returns None so the
    // caller treats it as a local path (git.ts:165-170).
    assert!(parse_git_url("github.com/user/repo").is_none());
    assert!(parse_git_url("just-a-name").is_none());
}

#[test]
fn git_url_hosted_shorthand_and_committish() {
    // hosted-git-info resolution path (git.ts:181-223). All values verified 1:1 against the real
    // npm `hosted-git-info@9.0.3` (the version Pi pins, package.json:51).

    // Bare GitHub shorthand `git:owner/repo` → https clone URL, github.com host, no ref.
    let bare =
        parse_git_url("git:owner/repo").expect("bare shorthand resolves via hosted-git-info");
    assert_eq!(bare.repo, "https://owner/repo");
    assert_eq!(bare.host, "github.com");
    assert_eq!(bare.path, "owner/repo");
    assert_eq!(bare.reff, None);

    // Host-shortcut + multi-segment user resolves through the gitlab table.
    let gl =
        parse_git_url("git:gitlab.com/group/sub/proj").expect("gitlab host-qualified resolves");
    assert_eq!(gl.repo, "https://gitlab.com/group/sub/proj");
    assert_eq!(gl.host, "gitlab.com");
    assert_eq!(gl.path, "group/sub/proj");

    // A `#committish` fragment on a known host becomes the (pinned) ref; the fragment stays on the
    // clone URL exactly as Pi keeps `split.repo` verbatim (repo includes `#v2`).
    let frag = parse_git_url("https://github.com/u/r#v2").expect("#committish resolves");
    assert_eq!(frag.repo, "https://github.com/u/r#v2");
    assert_eq!(frag.path, "u/r");
    assert_eq!(frag.reff.as_deref(), Some("v2"));
    assert!(frag.pinned, "an explicit #committish pins (git.ts:202/220)");
}

#[test]
fn git_url_security_rejects_traversal_and_injection() {
    // hasUnsafeGitInstallPart: path-traversal, leading-slash, NUL, and backslash are unsafe
    // (git.ts:84-102) — a SECURITY control.
    assert!(
        has_unsafe_git_install_part("..", true),
        ".. segment is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("a/../b", true),
        "embedded .. is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("/abs", true),
        "leading slash is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("a\\b", true),
        "backslash is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("%00", true),
        "encoded NUL is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("%2e%2e/x", true),
        "encoded .. is unsafe"
    );
    assert!(
        has_unsafe_git_install_part("a/b", false),
        "slash disallowed when allow_slash=false"
    );
    assert!(
        !has_unsafe_git_install_part("user", false),
        "plain segment is safe"
    );
    assert!(
        !has_unsafe_git_install_part("user/repo", true),
        "user/repo path is safe"
    );

    // The validator is wired into the parser. A *percent-encoded* `..` survives URL parsing as a
    // literal `..` segment and is rejected by hasUnsafeGitInstallPart → None (verified against Pi).
    assert!(parse_git_url("https://github.com/%2e%2e/secrets").is_none());
    // A *raw* `../` is collapsed by the WHATWG URL path machine BEFORE validation (new URL
    // normalizes `/../etc/passwd` → `/etc/passwd`), so Pi accepts it with a normalized, safe path —
    // there is no surviving `..` segment to reject (verified 1:1 against hosted-git-info@9.0.3).
    let normalized = parse_git_url("https://github.com/../etc/passwd").expect("raw .. normalized");
    assert_eq!(
        normalized.path, "etc/passwd",
        "raw .. collapsed by URL normalization, path is safe"
    );
    assert_eq!(normalized.host, "github.com");
}

#[test]
fn package_source_parse_routes_git_local_and_npm() {
    // Git URL → Git source carrying the clone URL (package-manager.ts:1417-1421).
    match PackageSource::parse("https://github.com/u/r").unwrap() {
        PackageSource::Git { url, reff } => {
            assert_eq!(url, "https://github.com/u/r");
            assert_eq!(reff, PinRef::Default);
        }
        other => panic!("expected Git, got {other:?}"),
    }
    // A hex ref pins as a commit; a named ref pins as a tag (both is_pinned, R-09-020 / git.ts:117).
    match PackageSource::parse("https://github.com/u/r@abc1234").unwrap() {
        PackageSource::Git { reff, .. } => {
            assert_eq!(reff, PinRef::Commit("abc1234".into()));
            assert!(reff.is_pinned());
        }
        other => panic!("expected Git commit pin, got {other:?}"),
    }
    match PackageSource::parse("https://github.com/u/r@release-1").unwrap() {
        PackageSource::Git { reff, .. } => assert_eq!(reff, PinRef::Tag("release-1".into())),
        other => panic!("expected Git tag pin, got {other:?}"),
    }
    // Bare names / relative paths → local Path (isLocalPath, paths.ts:41-55).
    assert!(matches!(
        PackageSource::parse("./pkg").unwrap(),
        PackageSource::Path { .. }
    ));
    assert!(matches!(
        PackageSource::parse("some-pkg").unwrap(),
        PackageSource::Path { .. }
    ));
    // npm channel dropped (R-09-021). CFG-009: the MESSAGE must name npm — this entry reaches the
    // user through settings `packages` on a normal session start, and the previous shared
    // `Unsupported` variant rendered as "unsupported source (OCI deferred)".
    let npm = PackageSource::parse("npm:foo@1.2.3");
    assert!(matches!(npm, Err(crate::ResourceError::UnsupportedNpm)));
    assert_eq!(
        npm.unwrap_err().to_string(),
        "unsupported source: npm packages are not supported"
    );

    // ParsedGitUrl::into_source round-trips (sanity for the public type).
    let parsed: ParsedGitUrl = parse_git_url("https://github.com/u/r").unwrap();
    assert!(matches!(parsed.into_source(), PackageSource::Git { .. }));
}

/// **CFG-052, REFUTED — this pins pi's ACTUAL behaviour, which cyrup already matches.**
///
/// CFG-052 asserts that "upstream's `parseGitUrl` reaches `hostedGitInfo.fromUrl`, which resolves
/// the `github:`/`gitlab:`/`bitbucket:` shorthands", and calls the resulting `PackageSource::Path`
/// an internal inconsistency created by porting two functions from two upstream files. Opening pi
/// at the ported tag refutes both halves:
///
/// - `parseGitUrl` (`utils/git.ts:172-179` @v0.83.0) opens with
///   `const hasGitPrefix = trimmed.startsWith("git:");` and
///   `if (!hasGitPrefix && !/^(https?|ssh|git):\/\//i.test(url)) { return null; }`. `github:u/r` has
///   no `git:` prefix, and the regex requires a literal `://`, so upstream **returns null before
///   `hostedGitInfo.fromUrl` is ever called** — the shorthand-resolution path CFG-052 relies on is
///   unreachable for this input. `crates/cyrup-resources/src/package/git_url.rs:278-287` is the same
///   two statements.
/// - The "inconsistency" is upstream's own and is deliberate: `parseSource`
///   (`core/package-manager.ts:1435-1459`) routes an `isLocalPath`-false string to `parseGitUrl` and
///   then falls through to the SAME `return { type: "local", path: source }` at `:1459` that
///   `isLocalPath`-true strings take at `:1450`. `isLocalPath` classifying `github:` as non-local
///   (`utils/paths.ts:41-55`) changes only which of the two identical returns is reached.
///
/// So a `github:` shorthand is a local path in pi and must stay a local path in cyrup. Encoding it
/// here — rather than leaving the case untested, as CFG-026 did — is what stops a future pass
/// "fixing" it into a divergence.
#[test]
fn cfg052_a_github_shorthand_is_a_local_path_exactly_as_upstream_leaves_it() {
    // Presence before absence: the same function DOES resolve the `git:`-prefixed shorthand, so a
    // `None` below is a statement about the missing `git:` prefix and not about a dead parser.
    let with_prefix = parse_git_url("git:owner/repo")
        .expect("`git:owner/repo` must still resolve through the hosted-git-info table");
    assert_eq!(with_prefix.host, "github.com");
    assert_eq!(with_prefix.path, "owner/repo");

    for shorthand in ["github:owner/repo", "gitlab:group/proj", "bitbucket:owner/repo"] {
        assert!(
            parse_git_url(shorthand).is_none(),
            "{shorthand}: pi's `parseGitUrl` returns null at git.ts:177-179 before reaching \
             hostedGitInfo, so cyrup's must too"
        );
        match PackageSource::parse(shorthand).unwrap() {
            PackageSource::Path { path } => assert_eq!(
                path,
                std::path::PathBuf::from(shorthand),
                "pi stores the source string VERBATIM on the local arm (package-manager.ts:1459)"
            ),
            other => panic!("{shorthand}: expected the local arm pi takes, got {other:?}"),
        }
    }

    // The identity keyer takes pi's same final `local:` arm, for the same reason.
    let base = std::path::Path::new("/base");
    assert_eq!(
        crate::package::package_identity("github:owner/repo", base),
        "local:/base/github:owner/repo"
    );
    // …while a form `parseGitUrl` CAN read keys on `git:<host>/<path>`, proving the `local:` arm
    // above is reached by the git parser declining, not by the identity keyer being inert.
    assert_eq!(
        crate::package::package_identity("git:owner/repo", base),
        "git:github.com/owner/repo"
    );
}
