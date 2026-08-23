//! Shared fixtures for the crate-internal test modules.
//!
//! Every leaf module under `tests/` needs the same three things: a temp dir that outlives the
//! session, a project `cwd` and an `agent_dir` inside it, and a `SessionConfig` pointing at both
//! with the trust prompt short-circuited. That block used to be pasted into 39 files verbatim, so
//! changing the fixture shape — adding a field, moving the temp-dir layout, changing a config
//! default — was a 39-file edit.
//!
//! This lives in the crate rather than in `cyrup-test-support` because of visibility, not because
//! of a dependency cycle: `SessionConfig`'s test-relevant fields and the `crate::` paths these
//! tests reach for are crate-internal, and `pub(super)` here (i.e. `pub(in crate::tests)`) is
//! visible to every sibling leaf module without widening the crate's public surface by one item.
//!
//! One fixture is deliberately NOT here — `read_image_auto_resize::ImageFixture` carries extra
//! fields only its own file uses. It is named apart from `Fixture` so the divergence is visible at
//! the use site.

use std::path::PathBuf;

use tempfile::TempDir;

use crate::SessionConfig;

/// A throwaway project tree: `<tmp>/project` as the cwd, `<tmp>/agent` as the agent dir.
///
/// `_tmp` is held only to keep the directory alive for the lifetime of the fixture; dropping the
/// `Fixture` deletes the tree.
pub(super) struct Fixture {
    pub(super) _tmp: TempDir,
    pub(super) cwd: PathBuf,
    pub(super) agent_dir: PathBuf,
}

pub(super) fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

/// The default config for a test session: pointed at the fixture's tree, with `trust_override`
/// standing in for `--approve` so no test blocks on the trust prompt.
pub(super) fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// [`base_config`] with on-disk extension auto-discovery switched off.
///
/// Used by the tests that assert over the extension/command registry: with discovery on, a stray
/// extension in the developer's own home directory can add or answer a command and change the
/// result. Only what the test registers explicitly is present.
pub(super) fn base_config_no_extensions(fx: &Fixture) -> SessionConfig {
    let mut cfg = base_config(fx);
    cfg.no_extensions = true;
    cfg
}
