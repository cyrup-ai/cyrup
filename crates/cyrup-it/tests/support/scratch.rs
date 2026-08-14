//! A tempdir-backed scratch HOME + agent dir for one test.
//!
//! §4 R1: every test owns a `tempfile::TempDir`; nothing writes to a shared, statically-named
//! path. That was free when each test was its own process with its own `$TMPDIR` conventions; in a
//! seven-binary suite it is the difference between an isolated test and a flaky one. The specific
//! violations this replaces: `std::env::temp_dir()/cyrup-session-svc-fixture-target` (10 files),
//! `std::env::temp_dir()/cyrup-ext-fixture-target` (5 files), and
//! `cyrup-ext/tests/loader.rs:9-18`'s `unique_dir()`, which names a directory with a nanosecond
//! stamp, no pid, and no cleanup — and whose three siblings' `std::process::id()` suffix stops
//! disambiguating anything the moment they share a binary.
//!
//! `CARGO_TARGET_TMPDIR` is deliberately NOT used anywhere in the workspace: it is set only when
//! building integration-test code, so it would break every one of the 199 files that moved into
//! `src/`. Zero files use it today; keep it that way.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// One test's private filesystem: a scratch `HOME`, an agent dir, and a working directory.
///
/// The `TempDir` is owned by this value, so the tree is removed when the test ends — keep the
/// `Scratch` alive for as long as any child process using it.
pub struct Scratch {
    root: TempDir,
}

impl Scratch {
    /// Create `home/`, `agent/` and `work/` under a fresh temp root.
    pub fn new() -> Self {
        let root = TempDir::new().expect("create scratch tempdir");
        for dir in ["home", "agent", "work"] {
            std::fs::create_dir_all(root.path().join(dir))
                .unwrap_or_else(|e| panic!("create scratch {dir}: {e}"));
        }
        Self { root }
    }

    /// The temp root itself. Prefer the named sub-directories.
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// The scratch `HOME` / `CYRUP_HOME`.
    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    /// The agent dir (`CYRUP_AGENT_DIR` / `CYRUP_CODING_AGENT_DIR`).
    pub fn agent_dir(&self) -> PathBuf {
        self.root.path().join("agent")
    }

    /// The child's working directory — the stand-in for "the user's project".
    pub fn work(&self) -> PathBuf {
        self.root.path().join("work")
    }

    /// Create a directory under the scratch root and return it.
    pub fn dir(&self, rel: impl AsRef<Path>) -> PathBuf {
        let path = self.root.path().join(rel);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
        path
    }

    /// Write a file under the scratch root, creating parents, and return its path.
    pub fn write(&self, rel: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
        }
        std::fs::write(&path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        path
    }

    /// A fully hermetic child rooted in this scratch: cleared environment, minimal allowlist,
    /// `HOME`/`CYRUP_HOME`/agent-dir pointed here, cwd = [`Scratch::work`].
    ///
    /// This is the one call a migrated test should need. It composes
    /// [`super::env::hermetic`] with this tree, replacing the eight hand-rolled hermetic-child
    /// builders in `crates/cyrup/tests` — one env-scrub bug in that family had to be fixed in
    /// **four** copies separately (`11-cyrup-intercom.md:791`).
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = super::env::hermetic(program, &self.home());
        cmd.env("CYRUP_HOME", self.home());
        cmd.env("CYRUP_AGENT_DIR", self.agent_dir());
        cmd.env("CYRUP_CODING_AGENT_DIR", self.agent_dir());
        cmd.current_dir(self.work());
        cmd
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Self::new()
    }
}
