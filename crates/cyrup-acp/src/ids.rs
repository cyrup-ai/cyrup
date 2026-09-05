//! The three values that arrive from the client as strings and become filesystem authorities.
//!
//! ADR-0028 finding F3 ([`AbsCwd`], [`SessionFile`]) plus the export-path half of `ACP-291`
//! ([`AcpSessionId`]). Step 2 of the ADR's migration plan: dependency-free apart from
//! [`crate::error`], and landed before any handler signature is fixed, because retrofitting a
//! newtype into signatures that already take `&Path` is the expensive direction.
//!
//! Ported from pi-acp v0.0.33 `src/acp/agent.ts`'s `isAbsolute(params.cwd)` guards, its
//! `deleteSession` / `cleanupFailedNewSession` `unlinkSync(sessionFile)` pair, and its
//! `safeSessionId` regex in the `/export` arm.
//!
//! # What these types do NOT guarantee — say it here, not in a review
//!
//! 1. **There is no validating `Deserialize` path.** `NewSessionRequest.cwd` is a `PathBuf`
//!    deserialized by `agent-client-protocol-schema` inside a `#[non_exhaustive]` struct cyrup does
//!    not own, before any cyrup code runs. So [`AbsCwd::parse`] must be the **first statement of
//!    each handler**, and the guarantee is "everything downstream of the handler", not
//!    "everything". None of these types implements `Deserialize`, `Default` or `From<PathBuf>`, so
//!    the only way to obtain one is to call the checked constructor.
//! 2. **[`SessionFile::resolve`]'s containment check is symlink-permeable.** `starts_with` after
//!    lexical normalisation does not resolve symlinks, and `canonicalize` fails on a path that does
//!    not exist yet, so the check cannot be both total and symlink-proof. A symlink *inside* the
//!    sessions root pointing outside it defeats it. This is stated rather than implied, and the
//!    test named `containment_is_symlink_permeable_and_that_is_documented` records the gap.
//! 3. **None of them says the path still exists**, or that another `cyrup` process is not writing
//!    it. This is a filesystem; TOCTOU is unaffected.

use std::path::{Component, Path, PathBuf};

use cyrup_session::layout::SessionsRoot;

use crate::error::{AcpError, AcpFailure};

/// Lexically resolve `.`/`..` in an already-absolute path **without touching the filesystem**.
///
/// Deliberately not `std::fs::canonicalize`: that fails on a path that does not exist yet, which is
/// exactly the case [`AcpSessionId::export_path_in`] is checking (the export file has not been
/// written). Mirrors `cyrup_config::trust`'s own `resolve_path` in shape — a `..` never escapes the
/// root — so the two agree about what a normalised path is.
fn normalize(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    out.iter().collect()
}

/// An absolute working directory supplied by the ACP client.
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `if (!isAbsolute(params.cwd)) throw
/// RequestError.invalidParams(...)` guard, hoisted to the type level so the third entry point
/// cannot skip it.
///
/// # [CYRUP-DELTA] — the guard covers three entry points, not two
///
/// **What differs.** Upstream checks `isAbsolute` in `newSession` and `loadSession` and **not** in
/// `restoreSession`, which every `session/prompt` reaches and which takes `cwd` from
/// `~/.pi/pi-acp/session-map.json`. A relative cwd from that file resolves every
/// `resolvePath(cwd, …)` in `session.ts` against the adapter's process cwd, so Zed opens the wrong
/// files. Here every function that joins against a cwd takes `&AbsCwd`, so the third path is
/// covered by construction.
///
/// **What it costs.** Nothing at runtime; the cost is discipline. Helper signatures take `&AbsCwd`
/// rather than `&Path`, and [`AbsCwd::as_path`] is called only at the actual I/O call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbsCwd(PathBuf);

impl AbsCwd {
    /// The only constructor.
    ///
    /// # Errors
    ///
    /// [`AcpFailure::InvalidParams`] with the message **byte-for-byte** from pi-acp v0.0.33
    /// `agent.ts` — `cwd must be an absolute path: <path>` (`ACP-056`, `ACP-211`).
    pub fn parse(path: impl Into<PathBuf>) -> Result<Self, AcpFailure> {
        let path = path.into();
        if path.is_absolute() {
            Ok(Self(path))
        } else {
            Err(AcpFailure::InvalidParams {
                message: format!("cwd must be an absolute path: {}", path.display()),
            })
        }
    }

    /// The path, for the actual I/O call and nowhere else.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Resolve a possibly-relative path against this cwd — pi-acp `session.ts`'s
    /// `isAbsolute(path) ? path : resolvePath(this.cwd, path)` (`ACP-130`). Taking `&self` is what
    /// makes the "resolved against a validated root" claim true.
    #[must_use]
    pub fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            normalize(path)
        } else {
            normalize(&self.0.join(path))
        }
    }
}

/// A session JSONL proven to live under the sessions root this process is configured for.
///
/// Port of the *authority* pi-acp v0.0.33 `session-store.ts`'s `StoredSession.sessionFile` carries
/// and never checks: upstream reads that string out of a JSON file whose loader validates only
/// `version === 1`, and hands it straight to `unlinkSync` in both `deleteSession` and
/// `cleanupFailedNewSession`. That is a delete primitive over an unvalidated path.
///
/// # [CYRUP-DELTA] — there is no sidecar to read a path out of
///
/// **What differs.** ADR-0028 §5 rejects mirroring `~/.pi/pi-acp/session-map.json` outright:
/// `cyrup_session::listing` plus `layout::{SessionLayout, SessionsRoot, encode_cwd}` give the same
/// facts in-process and typed, so [`SessionFile::from_listing`] is the constructor that is
/// *actually used* and [`SessionFile::resolve`] exists for a path that came from anywhere else.
///
/// **What it costs.** Upstream's reconciliation code (`findStoredSession`: try the store, fall back
/// to `findPiSession`, write the store back) has no port, so a client that relied on the sidecar
/// surviving a `cyrup` upgrade has nothing to migrate — there was never a cyrup sidecar. Deletion
/// takes the proof (`&SessionFile`) rather than a path, so no delete can be reached with a path
/// that did not come from either the listing layer or an explicit containment check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionFile(PathBuf);

impl SessionFile {
    /// Infallible: a listing entry is by construction under the root that produced it.
    ///
    /// The one constructor `cyrup-acp` normally uses, because `cyrup_session::listing` is the sole
    /// source of truth for the sessionId → path mapping.
    #[must_use]
    pub fn from_listing(info: &cyrup_session::listing::SessionInfo) -> Self {
        Self(info.path.clone())
    }

    /// Check a candidate path into the type: it must normalise to a `.jsonl` **under** `root`.
    ///
    /// # Errors
    ///
    /// [`AcpError::Path`] when the candidate is relative, escapes `root`, or does not end `.jsonl`.
    ///
    /// The extension check is not decoration: it is what stops a containment-passing path such as
    /// `<root>/settings.json` from reaching a delete. See the module docs for the symlink gap this
    /// check does **not** close.
    pub fn resolve(root: &SessionsRoot, candidate: &Path) -> Result<Self, AcpError> {
        if !candidate.is_absolute() {
            return Err(AcpError::Path(format!(
                "session file must be an absolute path: {}",
                candidate.display()
            )));
        }
        let normalized = normalize(candidate);
        let root_normalized = normalize(root.path());
        if !normalized.starts_with(&root_normalized) {
            return Err(AcpError::Path(format!(
                "session file is outside the sessions root: {}",
                candidate.display()
            )));
        }
        if normalized.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            return Err(AcpError::Path(format!(
                "session file must be a .jsonl: {}",
                candidate.display()
            )));
        }
        Ok(Self(normalized))
    }

    /// The path, for the actual I/O call.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// A session id received from an ACP client, checked before it is allowed near a path.
///
/// `ACP-291` — **this is a security control, not a cosmetic filter.** `session.sessionId` is not
/// agent-minted on every path: `session/load` takes the id from the client and every
/// `session/prompt` carries it, so an id containing `../` or an absolute-looking segment composes
/// straight into `join(cwd, …)` — and `PathBuf::join` with an absolute component **replaces** the
/// base, so `cwd.join("cyrup-session-/etc/x.html")` is `/etc/x.html`. On the cyrup side there is no
/// second line of defence: `AgentSession::export_to_html`
/// (`crates/cyrup-session-svc/src/session/transcript.rs`) takes the caller's path verbatim and ends
/// in a bare `std::fs::write`.
///
/// The check is `cyrup_session::validate_session_id` — pi's own `assertValidSessionId` — rather
/// than a re-derived regex, so the ACP boundary and the session layer agree by construction about
/// what a session id is.
///
/// **`ACP-Q45`/`ACP-Q46`, pre-decided and not reversible here:** `/export` does **not** accept a
/// client-supplied path, and containment is treated as a real boundary check rather than defence in
/// depth. [`AcpSessionId::export_path_in`] is the only constructor of an export path, and it
/// re-checks `parent() == Some(dir)` after composing, so the containment cannot be simplified away
/// into a bare `format!`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AcpSessionId(String);

impl AcpSessionId {
    /// The only constructor.
    ///
    /// # Errors
    ///
    /// [`AcpFailure::InvalidParams`] carrying `cyrup_session::validate_session_id`'s own message —
    /// pi's, verbatim — so an ACP client and the CLI's `--session-id` see the same sentence.
    pub fn parse(id: &str) -> Result<Self, AcpFailure> {
        cyrup_session::ids::validate_session_id(id)
            .map(|()| Self(id.to_string()))
            .map_err(|message| AcpFailure::InvalidParams { message })
    }

    /// The id as a string, for the wire and for lookups.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The `/export` output path for this session inside `dir`. `ACP-288` / `ACP-291`.
    ///
    /// `dir` is composed by the agent (the session cwd), never by the client — that is `ACP-Q45`.
    /// The filename is `cyrup-session-<id>.html`, matching pi-acp's
    /// `pi-session-${safeSessionId}.html` in shape.
    ///
    /// # Errors
    ///
    /// [`AcpError::Path`] when `dir` is not absolute, or when the composed path's parent is not
    /// `dir` after normalisation. The second check is redundant given [`AcpSessionId::parse`] has
    /// already rejected every character that could escape — and it is written anyway, because a
    /// containment guarantee that rests on a validator two functions away is one refactor from
    /// being untrue. It is the assertion, not the belt.
    ///
    /// # [CYRUP-DELTA] — overwriting is parity, and is not the regression
    ///
    /// A correct sanitiser still overwrites an existing `cyrup-session-<id>.html` in the user's
    /// project directory. pi-acp overwrote too; that half is recorded here rather than being
    /// treated as the defect, so a later reader does not mistake the two.
    pub fn export_path_in(&self, dir: &Path) -> Result<ExportPath, AcpError> {
        if !dir.is_absolute() {
            return Err(AcpError::Path(format!(
                "export directory must be an absolute path: {}",
                dir.display()
            )));
        }
        let dir = normalize(dir);
        let path = dir.join(format!("cyrup-session-{}.html", self.0));
        let normalized = normalize(&path);
        if normalized.parent() != Some(dir.as_path()) {
            return Err(AcpError::Path(format!(
                "export path escaped the session directory: {}",
                path.display()
            )));
        }
        Ok(ExportPath(normalized))
    }
}

/// A path `/export` is allowed to write to (`ACP-291`, `ACP-Q45`).
///
/// # Why this is a type and not a `PathBuf`
///
/// `ACP-Q45` and `ACP-Q46` are pre-taken and security-adjacent: `/export` accepts **no**
/// client-supplied path, the agent composes it inside the session cwd, and the containment check
/// is a real boundary rather than defence in depth. Both were honoured — and both were expressible
/// as a `PathBuf` that a one-line edit could replace with
/// `cwd.join(format!("cyrup-session-{id}.html"))`, with nothing in the tree noticing. That is the
/// arbitrary write `ACP-Q45` exists to prevent, one careless refactor away, in a function whose
/// own doc calls the recheck *"the assertion, not the belt"* — while the assertion did not exist.
///
/// The private field closes it: [`AcpSessionId::export_path_in`] is the only constructor in the
/// crate, so `crate::commands`' export arm cannot obtain a path to write to by any other route,
/// and substituting a bare join is a **compile error** rather than a silent hole. There is
/// deliberately no `From<PathBuf>`, no `new`, and no public field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPath(PathBuf);

impl ExportPath {
    /// The checked path, for the one call that writes to it.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// The file name, which is always present — [`AcpSessionId::export_path_in`] composes
    /// `<dir>/cyrup-session-<id>.html`.
    #[must_use]
    pub fn file_name(&self) -> Option<&std::ffi::OsStr> {
        self.0.file_name()
    }
}

impl std::fmt::Display for AcpSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&AcpSessionId> for agent_client_protocol::schema::v1::SessionId {
    fn from(id: &AcpSessionId) -> Self {
        agent_client_protocol::schema::v1::SessionId::new(id.0.clone())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// ACP-056 / ACP-211 — the message is byte-for-byte upstream's, at every entry point, because
    /// there is only one parser.
    #[test]
    fn abs_cwd_rejects_a_relative_path_with_upstreams_message() {
        assert_eq!(
            AbsCwd::parse("relative/path").unwrap_err(),
            AcpFailure::InvalidParams {
                message: "cwd must be an absolute path: relative/path".into()
            }
        );
        assert_eq!(
            AbsCwd::parse("").unwrap_err(),
            AcpFailure::InvalidParams {
                message: "cwd must be an absolute path: ".into()
            }
        );
        let ok = AbsCwd::parse("/tmp/project").unwrap();
        assert_eq!(ok.as_path(), Path::new("/tmp/project"));
    }

    /// ACP-130 — relative resolves against the session cwd, absolute passes through, and `..` is
    /// popped lexically so a tool-call location cannot point above the root by accident.
    #[test]
    fn abs_cwd_resolves_tool_call_locations_against_itself() {
        let cwd = AbsCwd::parse("/tmp/project").unwrap();
        assert_eq!(
            cwd.resolve(Path::new("src/main.rs")),
            Path::new("/tmp/project/src/main.rs")
        );
        assert_eq!(
            cwd.resolve(Path::new("/etc/hosts")),
            Path::new("/etc/hosts")
        );
        assert_eq!(
            cwd.resolve(Path::new("./a/../b")),
            Path::new("/tmp/project/b")
        );
    }

    fn root() -> (tempfile::TempDir, SessionsRoot) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = SessionsRoot(dir.path().to_path_buf());
        (dir, root)
    }

    /// The containment check, in both directions, including the `..` traversal that is the whole
    /// reason a bare `PathBuf` may not reach a delete.
    #[test]
    fn session_file_resolve_contains_and_requires_jsonl() {
        let (dir, sessions) = root();
        let inside = dir.path().join("proj").join("a.jsonl");
        assert_eq!(
            SessionFile::resolve(&sessions, &inside).unwrap().path(),
            inside.as_path()
        );
        // `..` out of the root, lexically — the delete primitive upstream ships.
        let escape = dir
            .path()
            .join("proj")
            .join("..")
            .join("..")
            .join("evil.jsonl");
        assert!(SessionFile::resolve(&sessions, &escape).is_err());
        // A relative candidate never resolves.
        assert!(SessionFile::resolve(&sessions, Path::new("a.jsonl")).is_err());
        // Contained but not a session file: the extension check is what stops this reaching a
        // delete.
        assert!(SessionFile::resolve(&sessions, &dir.path().join("settings.json")).is_err());
        // A sibling directory that merely shares a name prefix with the root is outside it.
        let sibling = dir.path().with_extension("evil").join("a.jsonl");
        assert!(SessionFile::resolve(&sessions, &sibling).is_err());
    }

    /// The gap, written as a test so it is a recorded fact rather than an assumption. ADR-0028 F3
    /// says to document this rather than imply the type proves more than it does.
    #[cfg(unix)]
    #[test]
    fn containment_is_symlink_permeable_and_that_is_documented() {
        let (dir, sessions) = root();
        let outside = tempfile::tempdir().expect("tempdir");
        let link = dir.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
        let through = link.join("a.jsonl");
        // `starts_with` after lexical normalisation does not resolve symlinks, so this PASSES —
        // and the file it names is outside the sessions root. `canonicalize` cannot be used
        // instead: it fails on a path that does not exist yet, which is the export case.
        assert!(
            SessionFile::resolve(&sessions, &through).is_ok(),
            "if this ever fails the gap has been closed — update the module docs"
        );
    }

    /// ACP-291 — the hostile-id cases, each of which is an arbitrary write under upstream's
    /// composition if the sanitiser is dropped.
    #[test]
    fn a_hostile_session_id_never_becomes_a_path() {
        for hostile in [
            "../../etc/passwd",
            "/etc/passwd",
            "..",
            ".",
            "a/b",
            "a\\b",
            "",
            " ",
            "-leading",
            "trailing-",
            "with space",
            "nul\0byte",
        ] {
            assert!(
                AcpSessionId::parse(hostile).is_err(),
                "`{hostile}` must not become an AcpSessionId"
            );
        }
        // The shapes cyrup actually mints: a uuid v7, and a `--session-id` friendly name.
        for ok in [
            "0199a2f1-7c3e-7a10-9f2b-2b0f1a5c9d33",
            "my_session",
            "my.session-1",
            "a",
        ] {
            assert!(AcpSessionId::parse(ok).is_ok(), "`{ok}` is a valid id");
        }
    }

    /// **ACP-291 / ACP-Q46** — the containment re-check is a real boundary, asserted with the id
    /// validator taken out of the picture.
    ///
    /// `export_path_in`'s own doc calls the re-check *"the assertion, not the belt"*, and until
    /// this test that was aspirational: `AcpSessionId::parse` rejects every character that could
    /// escape, so no input reachable through the public API can make the check fire, and neutering
    /// it to `if false` left the whole suite green. This test constructs the tuple struct directly
    /// — which only this module can do — so the belt is exercised on its own terms and deleting it
    /// is a failure rather than a silent widening.
    ///
    /// The ids below are exactly the ones `parse` refuses; the point is that if a second
    /// constructor ever appears, or `validate_session_id` is relaxed, `/export` still cannot write
    /// outside the session cwd.
    #[test]
    fn the_containment_recheck_holds_without_the_id_validator() {
        let dir = Path::new("/tmp/project");
        // Every one of these carries a path separator, which is the only way the composed
        // `cyrup-session-<id>.html` can stop being a direct child. A bare `..` is deliberately NOT
        // in the list: it composes `cyrup-session-...html`, an ordinary file name inside `dir`,
        // and refusing it would be the check misfiring rather than working.
        for hostile in [
            "../../etc/passwd",
            "a/b",
            "sub/../../escape",
            "/etc/passwd",
            "./x",
        ] {
            // Deliberately NOT `AcpSessionId::parse` — that is the other layer, and it is asserted
            // separately in `export_path_is_always_a_direct_child_of_the_session_cwd`.
            let unvalidated = AcpSessionId(hostile.to_string());
            let composed = unvalidated.export_path_in(dir);
            match composed {
                Err(AcpError::Path(message)) => assert!(
                    message.contains("escaped the session directory"),
                    "the refusal must name what it refused: {message}"
                ),
                Err(other) => panic!("expected a path refusal for `{hostile}`, got {other}"),
                Ok(path) => panic!(
                    "`{hostile}` composed `{}`, which is outside `{}` — this is the arbitrary \
                     write ACP-Q45 exists to prevent",
                    path.as_path().display(),
                    dir.display()
                ),
            }
        }

        // And the well-formed case is unaffected: the belt refuses escapes, not exports.
        let ok = AcpSessionId("2026-09-05_abc123".to_string());
        assert_eq!(
            ok.export_path_in(dir).unwrap().as_path(),
            Path::new("/tmp/project/cyrup-session-2026-09-05_abc123.html")
        );
    }

    /// ACP-291 — the export path is always a **direct child** of the session cwd, and the
    /// containment re-check is asserted independently of the id validator so that removing either
    /// one fails a test.
    #[test]
    fn export_path_is_always_a_direct_child_of_the_session_cwd() {
        let id = AcpSessionId::parse("my_session").unwrap();
        let cwd = Path::new("/tmp/project");
        let path = id.export_path_in(cwd).unwrap();
        assert_eq!(
            path.as_path(),
            Path::new("/tmp/project/cyrup-session-my_session.html")
        );
        assert_eq!(path.as_path().parent(), Some(cwd));
        // `PathBuf::join` with an absolute component REPLACES the base — the mechanism the whole
        // unit is about. It is unreachable through `AcpSessionId`, and this asserts that the
        // reason is the id, not luck.
        assert!(AcpSessionId::parse("/etc/x").is_err());
        // A relative export directory is refused rather than resolved against the process cwd.
        assert!(id.export_path_in(Path::new("relative")).is_err());
        // A cwd carrying `..` normalises before the parent check, so the check compares like with
        // like instead of failing on a spelling.
        let normalized = id.export_path_in(Path::new("/tmp/project/sub/..")).unwrap();
        assert_eq!(
            normalized.as_path(),
            Path::new("/tmp/project/cyrup-session-my_session.html")
        );
    }
}
