//! The script reference the description and the `models` errors point the model to, shipped inside
//! the binary.
//!
//! Upstream points at `<package dir>/docs/codemode.md` (`CODEMODE_DOCS_PATH`, `tool.ts:133`), a file
//! the npm package always contains. A cyrup binary installed on its own has no such directory
//! (`cyrup_config::docs_dir` finds one only next to a source checkout or under `CYRUP_ASSET_DIR`), so
//! the path the model was told to read did not exist in the user's project.
//!
//! [CYRUP-DELTA] The page is embedded in the binary and written under the agent directory when the
//! extension is attached, where the model can always read it.
//!
//! # Production call path
//!
//! `cyrup::session_launch::attach_native_extensions` calls
//! [`CodemodeExtension::with_agent_dir`](crate::CodemodeExtension::with_agent_dir) for every session,
//! which calls [`materialise_codemode_docs`] and hands the path to the tool's options.

use std::io;
use std::path::{Path, PathBuf};

use super::CODEMODE_DOCS_FILE;

/// `docs/codemode.md` as this binary was built with it.
pub const EMBEDDED_CODEMODE_DOCS: &str = include_str!("../../../../docs/codemode.md");

/// The directory under the agent directory that holds the materialised page.
const DOCS_DIR: &str = "docs";

/// Write [`EMBEDDED_CODEMODE_DOCS`] to `<agent_dir>/docs/codemode.md` unless that file already holds
/// it, and return the absolute path. A page left by an older build is replaced, so the model reads
/// the reference of the binary that is running; the new page is moved into place whole, so a reader
/// never sees half of it.
///
/// # Errors
///
/// The directory or the file could not be written.
pub fn materialise_codemode_docs(agent_dir: &Path) -> io::Result<PathBuf> {
    let dir = std::path::absolute(agent_dir)?.join(DOCS_DIR);
    let page = dir.join(CODEMODE_DOCS_FILE);
    if std::fs::read_to_string(&page).is_ok_and(|current| current == EMBEDDED_CODEMODE_DOCS) {
        return Ok(page);
    }
    std::fs::create_dir_all(&dir)?;
    let staging = dir.join(format!("{CODEMODE_DOCS_FILE}.{}.tmp", std::process::id()));
    let moved = std::fs::write(&staging, EMBEDDED_CODEMODE_DOCS)
        .and_then(|()| std::fs::rename(&staging, &page));
    if moved.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    moved.map(|()| page)
}

#[cfg(test)]
mod tests;
