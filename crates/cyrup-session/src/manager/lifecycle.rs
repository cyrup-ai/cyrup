//! Session construction (R-04-015/017/020/022/027/029): create, open (with the missing/zero-length
//! soft-new-session rules), continue-recent, fork and the in-memory variant. The tolerant reader
//! these lean on is [`super::load`]; the struct itself lives in [`super`].

use std::path::{Path, PathBuf};

use cyrup_core::SessionId;

use crate::error::SessionError;
use crate::header::SessionHeader;
use crate::ids::{gen_session_id, now_ts, validate_session_id};
use crate::layout::SessionLayout;
use crate::store::{DiskStore, MemStore, SessionStore};

use super::SessionManager;
use super::load::load;

/// Options for a new session.
#[derive(Clone, Debug, Default)]
pub struct NewSessionOpts {
    pub id: Option<SessionId>,
    pub parent_session: Option<String>,
}

impl SessionManager {
    /// Create a new persisted session for `cwd` (file deferred until the first assistant message).
    pub fn create(
        cwd: &Path,
        layout: &SessionLayout,
        opts: NewSessionOpts,
    ) -> Result<Self, SessionError> {
        let id = match opts.id {
            Some(id) => {
                validate_session_id(id.as_str()).map_err(SessionError::InvalidSessionId)?;
                id
            }
            None => gen_session_id(),
        };
        let ts = now_ts();
        let mut header = SessionHeader::new(id.clone(), cwd.to_string_lossy(), ts.clone());
        header.parent_session = opts.parent_session;
        let path = layout.new_file_path(&ts, id.as_str());
        let store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
        Ok(Self::assemble(
            header,
            cwd.to_path_buf(),
            store,
            Vec::new(),
            false,
        ))
    }

    /// Open an existing session by path, migrating to the current version on load (R-04-004).
    /// An empty/zero-length file initializes a fresh session at that path (Pi parity).
    pub fn open(path: &Path) -> Result<Self, SessionError> {
        Self::open_with_cwd(path, None)
    }

    /// Open an existing session by path, optionally overriding the manager's working directory
    /// (Pi `SessionManager.open(path, sessionDir?, cwdOverride?)`, session-manager.ts:1410): the
    /// effective cwd is `cwd_override ?? header.cwd` (Pi `cwdOverride ?? header?.cwd ?? process.cwd()`).
    /// The override rebinds only the manager's own cwd (what [`Self::cwd`] reports, used for cwd
    /// assertions and rebound services); the on-disk header retains its original cwd, exactly as Pi
    /// leaves `fileEntries`' header untouched (the override lives on the `this.cwd` field only).
    pub fn open_with_cwd(path: &Path, cwd_override: Option<&Path>) -> Result<Self, SessionError> {
        let meta = std::fs::metadata(path);
        // Pi treats a NONEXISTENT `--session <path>` and an EXISTING zero-length file identically:
        // both anchor a brand-new session at that exact path. `loadEntriesFromFile` returns `[]` for a
        // missing file (session-manager.ts:489-491), so `static open` reaches `setSessionFile`'s
        // `!existsSync` branch, which runs `newSession()` and preserves the explicit path
        // (session-manager.ts:843-847); an existing empty file takes the sibling `size === 0` branch
        // (session-manager.ts:822-831). Only a non-empty, unparseable file is an error. cyrup
        // previously special-cased only the zero-length case, so a missing path fell through to
        // `load()` → `File::open` → `NotFound` → a CLI hard-error (gap-analysis 05, Finding 2).
        let missing = matches!(&meta, Err(e) if e.kind() == std::io::ErrorKind::NotFound);
        if missing || matches!(&meta, Ok(m) if m.len() == 0) {
            // Missing/empty file → fresh session anchored here. The override (when given) seeds both
            // the header and the manager cwd, since there is no persisted header to preserve.
            let id = gen_session_id();
            // Pi: `const cwd = cwdOverride ?? (header ? getSessionHeaderCwd(header) : undefined)
            // ?? process.cwd();` (`session-manager.ts:1546`) — with no file there is no header, so
            // the fallback is the PROCESS cwd. `PathBuf::default()` is the EMPTY path, and
            // `newSession` writes it straight into the header (`:941`), producing `"cwd": ""` —
            // a session `session_cwd_matches` can never match, so it silently vanishes from every
            // cwd-filtered listing.
            let cwd = cwd_override
                .map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_default();
            let header = SessionHeader::new(id, cwd.to_string_lossy(), now_ts());
            let mut store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
            // The two branches differ upstream in exactly one respect. An EXISTING zero-length file
            // is rewritten with the fresh header immediately and marked flushed
            // (`session-manager.ts:907-911`: `this.newSession(); … this._rewriteFile();
            // this.flushed = true;`), because the file is already there. A MISSING file takes the
            // `else` branch (`:923-927`), which only calls `newSession()` — the write stays
            // deferred to the first assistant message. Treating both as deferred made the
            // zero-length case fail at that first flush: `create_exclusive` is pi's `"wx"` and
            // errors `AlreadyExists` on a file that already exists.
            let flushed = !missing;
            if flushed {
                store.rewrite(&header, &[])?;
            }
            return Ok(Self::assemble(header, cwd, store, Vec::new(), flushed));
        }
        let (mut header, mut entries, recovered) = load(path)?;
        let migrated = crate::migrate::to_current(&mut header, &mut entries);
        // Pi `cwdOverride ?? (header ? getSessionHeaderCwd(header) : undefined) ?? process.cwd()`
        // (`session-manager.ts:1546`), then `this.cwd = resolvePath(cwd)` (`:876`). An EMPTY header
        // cwd (written by an older cyrup — see the missing-file branch above) survives the `??`
        // chain upstream because `""` is a string, but `resolvePath("")` is
        // `nodeResolvePath(process.cwd(), "")` = the process cwd (`utils/paths.ts:81-85`), so pi
        // never runs with an empty `this.cwd`. The persisted header is left intact either way.
        let cwd = cwd_override
            .map(Path::to_path_buf)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| Some(PathBuf::from(&header.cwd)).filter(|p| !p.as_os_str().is_empty()))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let mut store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
        if migrated && !recovered {
            // Rewrite once at the current version (never silently discard a recovered prefix).
            store.rewrite(&header, &entries)?;
        }
        Ok(Self::assemble(header, cwd, store, entries, true))
    }

    /// Import an exported JSONL file and resume at its leaf (R-04-029). Equivalent to `open`.
    pub fn import_jsonl(path: &Path) -> Result<Self, SessionError> {
        Self::open(path)
    }

    /// Continue the most recent session for `cwd`, or create a new one if none exist (R-04-017).
    pub fn continue_recent(cwd: &Path, layout: &SessionLayout) -> Result<Self, SessionError> {
        Self::continue_recent_filtered(cwd, layout, false)
    }

    /// Continue the most recent session, optionally restricting the search to sessions whose header
    /// cwd matches `cwd` — Pi's `continueRecent` applies this filter when a custom `sessionDir` that
    /// is not the cwd-default is supplied (`filterCwd`, `session-manager.ts:1426-1434`), so a shared
    /// directory holding sessions from several projects only resumes the current one. With
    /// `filter_cwd = false` (the default layout case, where the dir already encodes the cwd) the
    /// behavior is identical to [`Self::continue_recent`].
    pub fn continue_recent_filtered(
        cwd: &Path,
        layout: &SessionLayout,
        filter_cwd: bool,
    ) -> Result<Self, SessionError> {
        let filter = if filter_cwd { Some(cwd) } else { None };
        match crate::listing::newest_session(&layout.dir(), filter) {
            Some(path) => Self::open(&path),
            None => Self::create(cwd, layout, NewSessionOpts::default()),
        }
    }

    /// An ephemeral session with no file persistence (R-04-027), validating a caller-supplied id
    /// 1:1 with Pi: `inMemory` constructs through the same constructor as a persisted session, whose
    /// `newSession` runs `assertValidSessionId` whenever an id is supplied
    /// (`session-manager.ts:830-831,1437-1439`) — so a malformed id is rejected for an ephemeral
    /// session exactly as for a persisted one. A `None` id is generated and never fails.
    pub fn in_memory(cwd: &Path, opts: NewSessionOpts) -> Result<Self, SessionError> {
        let id = match opts.id {
            Some(id) => {
                validate_session_id(id.as_str()).map_err(SessionError::InvalidSessionId)?;
                id
            }
            None => gen_session_id(),
        };
        let mut header = SessionHeader::new(id, cwd.to_string_lossy(), now_ts());
        header.parent_session = opts.parent_session;
        Ok(Self::assemble(
            header,
            cwd.to_path_buf(),
            Box::new(MemStore),
            Vec::new(),
            false,
        ))
    }

    /// Fork a source session into a new file under `target_cwd`, copying all source history
    /// verbatim and recording `parentSession` (R-04-020/022). The source is unchanged.
    pub fn fork_from(
        src: &Path,
        target_cwd: &Path,
        layout: &SessionLayout,
        opts: NewSessionOpts,
    ) -> Result<Self, SessionError> {
        let (_src_header, entries, _recovered) = load(src)?;
        if entries.is_empty() {
            return Err(SessionError::EmptyFork(src.to_path_buf()));
        }
        let id = match opts.id {
            Some(id) => {
                validate_session_id(id.as_str()).map_err(SessionError::InvalidSessionId)?;
                id
            }
            None => gen_session_id(),
        };
        let ts = now_ts();
        let mut header = SessionHeader::new(id.clone(), target_cwd.to_string_lossy(), ts.clone());
        header.parent_session = Some(
            opts.parent_session
                .unwrap_or_else(|| src.to_string_lossy().into_owned()),
        );
        let path = layout.new_file_path(&ts, id.as_str());
        let mut store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
        // Pi `forkFrom` writes the header with `{flag:"wx"}` (`session-manager.ts:1489`) — exclusive
        // create that refuses to clobber an existing file.
        store.create_exclusive(&header, &entries)?;
        Ok(Self::assemble(
            header,
            target_cwd.to_path_buf(),
            store,
            entries,
            true,
        ))
    }
}
