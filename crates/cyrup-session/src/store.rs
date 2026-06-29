//! Persistence seam (arch-04 §3.3). `DiskStore` = atomic append-only JSONL; `MemStore` = no-op
//! ephemeral backing (R-04-027). Keeps `SessionManager` logic identical for both.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::entry::Entry;
use crate::error::SessionError;
use crate::header::SessionHeader;

/// Abstraction over where a session's bytes live.
pub trait SessionStore: Send {
    /// `None` for in-memory (ephemeral) sessions.
    fn path(&self) -> Option<&Path>;
    fn is_persisted(&self) -> bool;
    /// Append ONE serialized entry line atomically (R-04-032). No-op in memory.
    fn append_line(&mut self, line: &str) -> Result<(), SessionError>;
    /// Rewrite the whole file (migration / clone eager-seed). No-op in memory.
    fn rewrite(&mut self, header: &SessionHeader, entries: &[Entry]) -> Result<(), SessionError>;
    /// Exclusive-create the file (header + entries), failing rather than clobbering a pre-existing
    /// file — Pi's `openSync(file,"wx")` first-flush and `writeFileSync … {flag:"wx"}` fork header
    /// (`session-manager.ts:927,1489`). This is the duplicate-header guard. No-op in memory.
    fn create_exclusive(
        &mut self,
        header: &SessionHeader,
        entries: &[Entry],
    ) -> Result<(), SessionError>;
}

/// Append-only JSONL file backing.
pub struct DiskStore {
    path: PathBuf,
}

impl DiskStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl SessionStore for DiskStore {
    fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }

    fn is_persisted(&self) -> bool {
        true
    }

    fn append_line(&mut self, line: &str) -> Result<(), SessionError> {
        // One `write` of `<json>\n` to an append-mode fd: a crash mid-write leaves a partial
        // final line that the tolerant reader drops ("last good line wins" — R-04-032).
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;
        f.sync_data()?;
        Ok(())
    }

    fn rewrite(&mut self, header: &SessionHeader, entries: &[Entry]) -> Result<(), SessionError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write to a temp sibling then atomically rename over the target, so a crash never leaves
        // a half-rewritten file.
        let tmp = self.path.with_extension("jsonl.tmp");
        let mut buf = String::new();
        buf.push_str(&serde_json::to_string(header)?);
        buf.push('\n');
        for e in entries {
            buf.push_str(&e.to_line()?);
            buf.push('\n');
        }
        {
            let mut f = File::create(&tmp)?;
            f.write_all(buf.as_bytes())?;
            f.flush()?;
            f.sync_data()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn create_exclusive(
        &mut self,
        header: &SessionHeader,
        entries: &[Entry],
    ) -> Result<(), SessionError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `create_new` is the atomic exclusive-create equivalent of Pi's `"wx"` flag: if the file
        // already exists it errors (`AlreadyExists`/EEXIST) instead of overwriting, guarding the
        // duplicate-header bug (`session-manager.ts:927,1489`). Written directly (no temp+rename)
        // to mirror Pi, which writes header+entries straight to the freshly-created fd.
        let mut f = match OpenOptions::new().write(true).create_new(true).open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(SessionError::AlreadyExists(self.path.clone()));
            }
            Err(e) => return Err(e.into()),
        };
        let mut buf = String::new();
        buf.push_str(&serde_json::to_string(header)?);
        buf.push('\n');
        for e in entries {
            buf.push_str(&e.to_line()?);
            buf.push('\n');
        }
        f.write_all(buf.as_bytes())?;
        f.flush()?;
        f.sync_data()?;
        Ok(())
    }
}

/// No-op backing for ephemeral sessions (R-04-027): never touches the filesystem.
pub struct MemStore;

impl SessionStore for MemStore {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn is_persisted(&self) -> bool {
        false
    }

    fn append_line(&mut self, _line: &str) -> Result<(), SessionError> {
        Ok(())
    }

    fn rewrite(
        &mut self,
        _header: &SessionHeader,
        _entries: &[Entry],
    ) -> Result<(), SessionError> {
        Ok(())
    }

    fn create_exclusive(
        &mut self,
        _header: &SessionHeader,
        _entries: &[Entry],
    ) -> Result<(), SessionError> {
        Ok(())
    }
}
