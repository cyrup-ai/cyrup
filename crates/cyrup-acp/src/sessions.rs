//! The one-live-session manager and the `session/*` entry points.
//!
//! **Owner: agent D (area 4d — `ACP-200`…`ACP-230`), plus `ACP-057`/`ACP-061`/`ACP-120` from 4c.**
//!
//! Port of pi-acp v0.0.33 `src/acp/session.ts`'s `class SessionManager` and of `src/acp/agent.ts`'s
//! `findStoredSession` / `restoreSession` / `listSessions` / `loadSession` / `deleteSession` /
//! `cleanupFailedNewSession` / `lastSessionCwd`.
//!
//! # [CYRUP-DELTA] — a slot, not a map, and it is structural
//!
//! **What differs.** Upstream's `SessionManager` is a genuine `Map<string, PiAcpSession>` of N live
//! child processes, and its `closeAllExcept` is leak avoidance it could drop tomorrow. cyrup's
//! `AgentSessionRuntime` (`crates/cyrup-session-svc/src/runtime.rs`) holds one `Arc<AgentSession>`
//! in one slot behind one generation watch — it is a replacer, not a multiplexer.
//!
//! **What it costs.** N sessions in one process are unsafe today for reasons this crate does not
//! own: `NativeExtension::set_host_services` stashes the host-services `Arc` in **first-write-wins
//! `OnceLock` slots** inside `PermissionSystemExtension`, `McpExtension` and `FluxExtension`, so
//! session B's permission dialog would open on session A's `UiSink`; and `RUNTIME_API`
//! (`crates/cyrup-permission-system/src/runtime_api.rs`) and `ROOT_PARENT_SESSION_ANCHOR`
//! (`crates/cyrup-ext-subagents/src/background/parent_anchor.rs`) are process-global
//! last-writer-wins slots on the permission path. In practice Zed opens one ACP connection per
//! project window and this is invisible; a client that opens two workspaces on one connection gets
//! B evicting A. `ACP-061` asserts the eviction is observable rather than silent, and `ACP-154`
//! guarantees the evicted session's in-flight `session/prompt` receives a response rather than
//! hanging.
//!
//! # No `session-map.json`, and that is decided
//!
//! ADR-0028 §5 rejects mirroring `~/.pi/pi-acp/session-map.json` outright: cyrup mints the session
//! id and names the file after it (`SessionLayout::new_file_path(ts, uuid)` is called with
//! `id.as_str()` at all three creation sites), so the sessionId → path map is derivable and
//! `cyrup_session::listing` is the single source of truth.
//!
//! # `ACP-222`, decided — the filename is a **hint**, the header is the **authority**
//!
//! `ACP-222` is the unit that says the derivation the sidecar cut rests on is unsound today, and it
//! had to land before `ACP-202` became the sole restore path. It is measured, not asserted:
//! `cyrup_session::ids::validate_session_id` permits `.`, `_` and `-` in the interior of an id;
//! `SessionLayout::new_file_path` writes `<sanitized-ts>_<id>.jsonl`; and
//! `cyrup_session::listing`'s private `uuid_of` splits on the **last** underscore with a
//! whole-stem fallback — so `--session-id my_session` derives `"session"` and `s.jsonl` derives
//! `"s"`. Both shapes appear in pi-acp's own fixtures (`0000_delete_me.jsonl` derives `"me"`).
//!
//! `ACP-222` offers two fixes: repair `uuid_of`, or keep header-id resolution authoritative with
//! the filename as a confirmed fast-path hint. **The second is taken**, for two reasons that are
//! not preference. `uuid_of` lives in `cyrup-session`, which this module does not own and whose
//! `listing::resolve` is on the CLI's `--session` path — changing it changes what
//! `cyrup --session my_session` opens for every existing user. And repairing it to `split_once`
//! would still be a filename derivation, i.e. still a guess about a name a user can choose, where
//! `parseSessionHeader`'s `obj.id` is the fact upstream actually matches on.
//!
//! So [`find_stored`] scans the sessions root, **orders** the candidates by a filename hint, and
//! confirms every candidate by reading its header. `ACP-202`'s cost requirement holds: it opens
//! files but parses **no session bodies** — [`read_header_of`] is the same bounded first-parsed-entry
//! scan `listing::read_header` performs, capped at 1 MiB and stopping at the first parsed line.
//!
//! **What it costs.** A *miss* reads one header per `*.jsonl` under the root rather than zero.
//! That is still strictly cheaper than upstream's `findPiSession`, which reads the first line
//! **and** a 256 KiB tail of every file for the same answer, and it is the price of never opening
//! the wrong transcript.
//!
//! # `ACP-223` / `ACP-Q31`, decided — one level of descent, always
//!
//! `session_list_layout` picks `SessionLayout::literal` for a settings-derived `sessionDir` and
//! `list_global_sessions` then scans it **flat**, while `listing::list_all` scans only
//! `<root>/*/` and never the root itself. Upstream's own fixture
//! (`test/component/session-list-custom-session-dir.test.ts`) writes
//! `settings.json {sessionDir}` with the session one cwd-encoded level **below** it and asserts it
//! is found, and `ACP-223` records that cyrup returns zero sessions there.
//!
//! [`session_dirs`] takes the third option this module can take without editing `cyrup-session`:
//! **the root itself plus every immediate subdirectory**, always, for both listing and resolution.
//! That is exactly the two-level shape `walkJsonlFiles` walks, it finds the flat files an explicit
//! `--session-dir` writes *and* the cwd-encoded ones written before the setting existed, and it
//! makes `ACP-Q31`'s "flat scan plus cwd filter" prescription unnecessary rather than wrong.
//!
//! **What it costs.** One `read_dir` of the root per listing, and a session buried two levels down
//! is still invisible. Nothing in cyrup's layout writes one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, DeleteSessionRequest, DeleteSessionResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, Meta, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, SessionId, SessionInfo as AcpSessionInfo,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse, ToolCallContent,
    ToolCallId as AcpToolCallId,
};
use agent_client_protocol::{BoxFuture, Client, ConnectionTo, Responder};
use cyrup_core::{Content, Message};
use cyrup_session::agent_message::AgentMessage;
use cyrup_session::header::SessionHeader;
use cyrup_session::layout::SessionsRoot;
use cyrup_session::listing::SessionInfo as StoredInfo;
use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, InputSource, ReplayItem,
    SessionServiceError, SessionTarget, UiEffectSink, UiSink, UserInput, delete_session_file_at,
};
use futures::StreamExt as _;
use serde_json::Value;

use crate::HandlerOutcome;
use crate::connection::ClientView;
use crate::error::{AcpError, AcpFailure};
use crate::ids::{AbsCwd, AcpSessionId, SessionFile};
use crate::ledger::{Announce, FileSnapshot, ToolCallLedger, ToolClass, ToolStatus, UpdatePatch};
use crate::permission::{DialogCaps, DialogClient, PermissionBridge};
use crate::translate::bash_exit_code;
use crate::turn::{RunStarted, RuntimeAgent, TurnAgent, TurnHandle, TurnSink};
/// What [`AcpHost::build_runtime`] is asked for.
#[derive(Clone, Debug)]
pub struct RuntimeRequest {
    /// The session's working directory, already checked absolute at the handler boundary
    /// ([`AbsCwd`], `ACP-056`/`ACP-211`).
    pub cwd: AbsCwd,
    /// Which session to attach to: `New` for `session/new`, `Resume(path)` for `session/load` and
    /// for the `session/prompt` restore path.
    pub target: SessionTarget,
}

/// The session-building capability that lives in the `cyrup` **binary** and cannot be reached from
/// a library crate.
///
/// # Why this trait exists — `ACP-Q30`, decided
///
/// `crate::session_launch::build_factory`, `crate::signals::spawn_abort_on_signal` and the four
/// listing helpers `session_list_layout` / `session_list_cwd_filter` / `gather_session_refs` /
/// `list_global_sessions` are all in the `cyrup` bin crate, which **depends on** `cyrup-acp` — so
/// the dependency cannot be inverted and this crate cannot call them. `ACP-Q30` offered three
/// answers: lift them into `cyrup-session`, put the ACP mode inside `crates/cyrup`, or invert the
/// call. **This trait is the third**: the binary implements it and hands one in, so `cyrup-acp`
/// stays a library with a unit-testable seam and no lift is required.
///
/// **What it costs.** Two things, stated so they are not rediscovered. (1) The four listing helpers
/// are still unreachable, so `ACP-200`/`ACP-201`/`ACP-207`/`ACP-223` must be written against
/// `cyrup_session::listing::{list_all, list_in_dir}` and `layout::{SessionLayout, encode_cwd}`
/// directly rather than reusing the bin's wrappers — which is the better source anyway, since those
/// wrappers add CLI-shaped policy (`--resume` picker ordering) the ACP host does not want.
/// (2) There is one indirection between a handler and the factory, so a test double
/// ([`null_host`]) can stand in for the whole binary — which is what makes this module testable at
/// all.
///
/// # `ACP-003` / `ACP-023` — the runtime is built LAZILY
///
/// `build_runtime` is called from `session/new`, not from `main`. `run_rpc` opens with
/// `runtime.session().await.bind_extensions().await` (SEAM-033 — the host announces after
/// `--name`/`--models`), but ACP must announce after `initialize` settles, because `has_ui` and the
/// client's advertised capabilities are what a `session_start` handler should see. So `main` builds
/// only the (cheap) `SessionFactory` and this trait defers the rest.
pub trait AcpHost: Send + Sync + 'static {
    /// Build a live runtime for `req`, or fail.
    ///
    /// # Errors
    ///
    /// [`AcpError::Session`] carrying the typed `SessionServiceError` — trust refusal,
    /// `MissingSessionCwd`, extension-host load failures — so [`AcpFailure::classify`] can still
    /// decide at the boundary whether the client sees `-32000` or `-32603` (`ACP-058`).
    ///
    /// **This is where pi-acp's spawn-diagnostic failure class went.** There is no child, so
    /// `PiRpcSpawnError` and its three ENOENT/EACCES/other messages have no counterpart and must
    /// not be reintroduced: `ACP-001` re-enters the current executable, so ENOENT is structurally
    /// impossible.
    fn build_runtime<'a>(
        &'a self,
        req: &'a RuntimeRequest,
    ) -> BoxFuture<'a, Result<Arc<AgentSessionRuntime>, AcpError>>;

    /// Called once with each newly built runtime, before it serves anything.
    ///
    /// `ACP-023` — this is the runtime handoff the signal watcher needs.
    /// `crate::signals::spawn_abort_on_signal(runtime, cancel, host)` takes the runtime **by value
    /// at spawn time**, and `ACP-006` prescribes arming it in `main`'s ACP arm exactly as the `Rpc`
    /// arm does — but the runtime does not exist in `main`. The two cannot both hold, so:
    ///
    /// **Decision: arm on the first `session/new`.** The alternative (a `watch`/`OnceLock` handoff
    /// into a watcher armed in `main`) buys nothing here, because the watcher's documented **first
    /// act** — `kill_tracked_detached_children()`, *"genuinely first: before the repeat watcher,
    /// before the abort, before the dispose"* — has nothing to kill before a session exists: no
    /// session means no tracked bash process group and no runtime to dispose.
    ///
    /// **What it costs, precisely.** A SIGTERM delivered between `initialize` and the first
    /// `session/new` is handled by tokio's default disposition rather than by the watcher, so the
    /// process dies without emitting `session_shutdown{reason:"quit"}` — which no extension is
    /// listening for yet, because no session has been announced. The window is real and it is empty.
    fn runtime_ready(&self, runtime: &Arc<AgentSessionRuntime>);

    /// The configured sessions root, for `session/list` and for [`crate::ids::SessionFile`]'s
    /// containment check. This is `ConfigDirs::session_dir` with the CLI > env > settings ladder
    /// already applied by the binary (`ACP-200`), which is why it is asked for rather than
    /// re-derived: re-deriving it here would drop the `--session-dir` tier.
    fn sessions_root(&self) -> SessionsRoot;
}

/// A host that builds nothing, for unit tests and for the `AcpConnection` constructor's own tests.
///
/// Not a `Default` stub standing in for a decision: it is a genuine test double whose every method
/// is an honest answer for "there is no binary here". It is `pub` because `cyrup-it` needs it to
/// drive the handler table without a real session.
#[must_use]
pub fn null_host() -> Arc<dyn AcpHost> {
    struct NullHost;
    impl AcpHost for NullHost {
        fn build_runtime<'a>(
            &'a self,
            _req: &'a RuntimeRequest,
        ) -> BoxFuture<'a, Result<Arc<AgentSessionRuntime>, AcpError>> {
            Box::pin(async {
                Err(AcpError::Host(
                    "no session host is installed on this connection".into(),
                ))
            })
        }
        fn runtime_ready(&self, _runtime: &Arc<AgentSessionRuntime>) {}
        fn sessions_root(&self) -> SessionsRoot {
            SessionsRoot(PathBuf::new())
        }
    }
    Arc::new(NullHost)
}

/// `cyrup_session::listing::scan_file`'s "this session has no user messages" sentinel, which it
/// puts in `SessionInfo.first_message` rather than leaving the field empty.
///
/// `ACP-205` maps it to `title: null`. See [`title_of`] for the ambiguity that carries: a session
/// whose first user message is literally `(no messages)` reports no title. That is inherent to a
/// sentinel living in the same field as the data, and closing it means changing
/// `cyrup_session::listing::SessionInfo`, which is not this port's to reshape.
const NO_MESSAGES_SENTINEL: &str = "(no messages)";

/// pi-acp v0.0.33 `agent.ts`'s `PAGE_SIZE` in `listSessions` (`ACP-208`).
const PAGE_SIZE: usize = 50;

/// The header of a session file, or `None` if the file's first parsed entry is not one.
///
/// `ACP-202`'s bounded read, and it is [`cyrup_session::listing::read_header`] — **not** a copy of
/// it. That matters more than the four lines it saves: `read_header` shares its first-entry rule
/// (`header_candidate`) with `listing::scan_file` and `cyrup_session::manager::load`, and the rule
/// was extracted precisely because those three once disagreed about whether a file with a leading
/// blank line was a session at all. A private re-implementation here would put the ACP host back on
/// the wrong side of that disagreement — `session/list` and `session/load` would answer differently
/// about the same file — which is why the integration phase exported the original rather than
/// keeping the copy this module shipped with.
fn read_header_of(path: &Path) -> Option<SessionHeader> {
    cyrup_session::listing::read_header(path)
}

/// Every directory a session file may live in under `root`: the root itself, then each immediate
/// subdirectory (`ACP-223` / `ACP-Q31`, decided — see the module docs).
///
/// # `ACP-229` / `ACP-200`, decided — a relative sessions root is refused, not anchored
///
/// Upstream's `readSessionDirFromSettings` anchors a relative `sessionDir` to the agent dir
/// (`~/.pi`), so `{"sessionDir": "sessions-alt"}` means `~/.pi/sessions-alt`. cyrup does not
/// absolutize: `ConfigDirs` stores what it was given, and this module receives it through
/// [`AcpHost::sessions_root`], which is where the CLI > env > settings ladder has already been
/// applied (`ACP-200`) and is why it is asked for rather than re-derived — re-deriving it here
/// would drop the `--session-dir` tier.
///
/// **The decision: an empty listing, not a scan of the process cwd.** A relative root passed to
/// `read_dir` resolves against *this process's* working directory, which for an ACP host is
/// wherever the editor happened to spawn it — the precise "wrong root" failure mode
/// [`AbsCwd`] exists to make unrepresentable, and one that would silently list, load and **delete**
/// files from an unrelated tree. Anchoring it to the agent dir instead is not this module's call:
/// it would mean `cyrup --acp` and `cyrup` disagreeing about where the same setting points.
///
/// **What it costs.** A user whose settings name a relative `sessionDir` sees an empty
/// `session/list` under ACP where upstream would have listed `~/.pi/<dir>`. That is visible and
/// diagnosable; the alternative is silent and destructive.
fn session_dirs(root: &SessionsRoot) -> Vec<PathBuf> {
    let base = root.path();
    if !base.is_absolute() {
        tracing::warn!(
            root = %base.display(),
            "ACP-229: the configured sessions root is not absolute; session/list and session/load \
             will find nothing rather than scanning this process's working directory"
        );
        return Vec::new();
    }
    let mut out = vec![base.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            // `is_dir` follows symlinks, so a symlinked project directory is descended into —
            // `ACP-230`'s decision, recorded on `find_stored`.
            if path.is_dir() {
                out.push(path);
            }
        }
    }
    // `read_dir` order is filesystem order. Sorting makes a listing reproducible across runs and
    // makes `find_stored`'s candidate order deterministic, which is what its test rests on.
    out.sort();
    out
}

/// Where a stored session lives, resolved from the sessions root and nothing else.
///
/// Port of the *return value* of pi-acp v0.0.33 `agent.ts`'s `findStoredSession` —
/// `{ cwd, sessionFile }` — with the sidecar half deleted (ADR-0028 §5) and the id carried along
/// so a caller cannot pair a file with the wrong id.
#[derive(Clone, Debug)]
pub struct StoredSession {
    /// The id, already through [`AcpSessionId::parse`].
    pub id: AcpSessionId,
    /// The JSONL, proven to be under the sessions root (`SessionFile`, ADR-0028 F3).
    pub file: SessionFile,
    /// The session's own recorded working directory, or `None` when the header carries none or
    /// carries a relative one.
    ///
    /// Upstream's `stored.cwd` is a `string` read out of the sidecar and is never checked — which
    /// is the third `isAbsolute` entry point `AbsCwd`'s doc records as missing upstream. Here it is
    /// `None` rather than a path that would resolve against this process's cwd.
    pub cwd: Option<AbsCwd>,
}

/// Whether `path`'s filename derivation *might* name `id` — a hint that orders the scan and is
/// never trusted (`ACP-222`, see the module docs).
///
/// Mirrors `cyrup_session::listing`'s private `uuid_of` (`<stem>.rsplit_once('_')` with a
/// whole-stem fallback) **as a predicate**: both of its answers are accepted as a hint, because
/// both of its answers can be wrong.
fn filename_hints_at(path: &Path, id: &str) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    stem == id
        || stem
            .strip_suffix(id)
            .is_some_and(|head| head.ends_with('_'))
}

/// Resolve one session id to its file and recorded cwd, reading **no session bodies**
/// (`ACP-201`, `ACP-202`, `ACP-222`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `findStoredSession` and `pi-sessions.ts`'s `findPiSession`,
/// with the store lookup and the store write-back deleted: there is no sidecar to reconcile, so the
/// "try the store, fall back to a scan, write the store back" triple collapses to the scan.
///
/// The scan covers the root and one level below it ([`session_dirs`]), which is what makes this
/// **local then cross-project** in one pass — `ACP-201`'s two-tier lookup, where the local tier is
/// simply the cwd-encoded directory the hint happens to find first.
///
/// # `ACP-230`, decided — a symlinked session file is resolved and listed
///
/// Upstream's `walkJsonlFiles` uses `lstat` predicates and skips symlinks entirely; cyrup's
/// `collect_paths` does not, and neither does this. **The decision is to keep cyrup's behaviour**:
/// a user who symlinks a session into their sessions root did so in order to see it, and the two
/// front-ends must not disagree about which sessions exist. The cost is stated at
/// [`SessionFile::resolve`] rather than here — containment is lexical, so a symlink *inside* the
/// root pointing outside it passes the check. `ACP-Q46` is answered "real boundary check", and this
/// is the one hole in it that is documented rather than closed.
#[must_use]
pub fn find_stored(root: &SessionsRoot, id: &AcpSessionId) -> Option<StoredSession> {
    let mut hinted: Vec<PathBuf> = Vec::new();
    let mut rest: Vec<PathBuf> = Vec::new();
    for dir in session_dirs(root) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if filename_hints_at(&path, id.as_str()) {
                hinted.push(path);
            } else {
                rest.push(path);
            }
        }
    }
    hinted.sort();
    rest.sort();

    for path in hinted.into_iter().chain(rest) {
        let Some(header) = read_header_of(&path) else {
            continue;
        };
        if header.id.as_str() != id.as_str() {
            continue;
        }
        let Ok(file) = SessionFile::resolve(root, &path) else {
            // Unreachable for a path that came out of `read_dir` on the root, and not treated as
            // the end of the search if the impossible happens.
            continue;
        };
        return Some(StoredSession {
            id: id.clone(),
            file,
            cwd: AbsCwd::parse(header.cwd).ok(),
        });
    }
    None
}

/// Every stored session under `root`, newest first, optionally restricted to one cwd
/// (`ACP-203`, `ACP-207`).
///
/// One streaming pass per directory through `cyrup_session::listing::list_in_dir`, which **is**
/// pi-acp's `listPiSessions` and its five helpers: `scan_file` already applies the same
/// first-parsed-entry header rule, already takes the newest non-blank `session_info.name`
/// (including explicit clears — `ACP-228`), already falls back from the last message activity time
/// to the header time to the file mtime for `modified` (upstream's `pickUpdatedAtFromTail` →
/// `statSync().mtime`), and already sorts newest-first. **None of that is re-ported**; the five
/// scanning strategies upstream needs (first line, 64 KiB head, 256 KiB tail, whole-file rescan,
/// second whole-file read) collapse to one because cyrup reads its own format.
///
/// `cwd_filter` is `list_in_dir`'s own, i.e. `session_cwd_matches`, which is upstream's strict
/// compare — applied **before** the page slice, because `ACP-208`'s `nextCursor` is computed
/// against the filtered length and a filter applied after the slice silently shrinks pages.
///
/// # [CYRUP-DELTA] — the cwd compare is `Path`-equality, not string equality
///
/// **What differs.** Upstream is `s.cwd === effectiveCwd`, a raw string compare with no
/// normalization and no trailing-slash tolerance. `session_cwd_matches` compares
/// `Path::new(session_cwd) == resolved_cwd`, which is component-wise, so `/a/b/` and `/a/b` match
/// here and do not match upstream.
///
/// **What it costs.** Strictly more rows than upstream for a client that sends a trailing slash,
/// and no row upstream would have returned that this drops. That direction is the safe one for a
/// filter whose failure mode is an empty session picker.
fn list_rows(root: &SessionsRoot, cwd_filter: Option<&Path>) -> Vec<StoredInfo> {
    let mut all: Vec<StoredInfo> = Vec::new();
    for dir in session_dirs(root) {
        all.extend(cyrup_session::listing::list_in_dir(&dir, cwd_filter, None));
    }
    all.sort_by_key(|s| std::cmp::Reverse(s.modified));
    all
}

/// The `title` for one listing row (`ACP-205`).
///
/// Port of pi-acp v0.0.33 `pi-sessions.ts`'s `pickTitleFromTail` → `scanSessionInfoNameFromFile` →
/// `pickFallbackTitleFromHead` ladder. The first two rungs are `SessionInfo.name`, which
/// `listing::scan_file` already computes with the same "newest non-blank trimmed wins" rule; the
/// third is the first user message clipped to 80 characters; the fourth is `null`.
///
/// # [CYRUP-DELTA] — the sentinel, the clip, and the join
///
/// **What differs, three ways.** (1) `scan_file` substitutes the literal `"(no messages)"` when it
/// found no user message with text; that is a TUI display string and must never reach the wire, so
/// it maps to `None`. (2) `first_message` is untruncated, so the 80-character clip is applied here
/// — `chars().take(80)`, which counts `char`s where JS `slice` counts UTF-16 code units, so an
/// astral-plane character costs one here and two there. (3) `scan_file` joins a user message's text
/// blocks with `" "` where upstream takes only the first block, so a multi-block first message
/// yields a longer title before the clip.
///
/// **What it costs.** A session whose first user message is literally `(no messages)` reports no
/// title. That ambiguity is inherent to a sentinel carried in the same field as the data, it is
/// upstream's own shape (`pickFallbackTitleFromHead` has no sentinel to confuse), and closing it
/// means changing `listing::SessionInfo`, which this module does not own.
fn title_of(info: &StoredInfo) -> Option<String> {
    if let Some(name) = info
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        return Some(name.to_string());
    }
    let first = info.first_message.trim();
    if first.is_empty() || first == NO_MESSAGES_SENTINEL {
        return None;
    }
    Some(info.first_message.chars().take(80).collect())
}

/// `SystemTime` as JS `Date.prototype.toISOString()` — `YYYY-MM-DDTHH:MM:SS.mmmZ` (`ACP-204`).
///
/// Port of pi-acp v0.0.33 `pi-sessions.ts`'s two `new Date(...).toISOString()` sites and its
/// `statSync(file).mtime.toISOString()` fallback. Millisecond precision and a literal `Z` are both
/// load-bearing: `ACP-204`'s verify is a regex over the emitted string, and a client parsing
/// `+00:00` where it expects `Z` is a real class of bug.
///
/// Written out rather than pulled from `time`: `cyrup-acp`'s dependency list is not this module's
/// to edit, and the civil-from-days conversion is a closed-form integer algorithm with a test.
/// `None` for a time before the Unix epoch, which upstream renders as a negative-year ISO string
/// and which no session file can produce.
fn iso8601_millis(time: SystemTime) -> Option<String> {
    let since = time.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let secs = i64::try_from(since.as_secs()).ok()?;
    let millis = since.subsec_millis();
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    ))
}

/// Now, in the same `YYYY-MM-DDTHH:MM:SS.mmmZ` shape [`iso8601_millis`] gives `session/list`'s
/// rows.
///
/// # [CYRUP-DELTA] — one `updatedAt` format, not two
///
/// **What differs.** `session_info_update`'s `updatedAt` was built from
/// `cyrup_session::ids::now_ts()`, which is a bare `OffsetDateTime::now_utc().format(Rfc3339)` and
/// therefore emits **nine** fractional digits — `2026-09-05T08:39:44.722877348Z` — while
/// `session/list`'s rows emit three. Both are legal ISO 8601, so nothing failed to deserialize;
/// what a client sees is two formats for one field, and `session_info_update` is precisely the
/// notification that updates a row `session/list` produced.
///
/// **What it costs.** Nothing observable except the six digits: `ACP-204`'s verify is a regex that
/// pins the three-digit form, so this is the format the port already committed to. `now_ts()` is
/// still the right call everywhere it is used for a session *entry* timestamp; this is only the
/// wire.
///
/// The `unwrap_or_default` is unreachable — [`iso8601_millis`] returns `None` only for a time
/// before the Unix epoch — and yields the empty string rather than panicking on a clock that
/// somehow is.
#[must_use]
pub fn now_iso8601_millis() -> String {
    iso8601_millis(SystemTime::now()).unwrap_or_default()
}

/// Howard Hinnant's `civil_from_days` — days since the Unix epoch to `(year, month, day)`.
///
/// The closed form, so [`iso8601_millis`] needs no calendar table and no dependency. Only the
/// non-negative branch is reachable from a `SystemTime` after the epoch; the negative branch is
/// written because omitting it would make the function wrong rather than partial.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// One ACP `session/list` row, or `None` for a session ACP cannot describe (`ACP-206`).
///
/// `SessionInfo.cwd` is a **required absolute path** on the wire, and cyrup's header cwd is
/// read-tolerant: `de_string_or_empty` lands an absent or non-string `cwd` as `""` so the session
/// stays listable in the TUI. A row with no usable cwd is therefore present in
/// `listing::list_all` (cyrup's tolerance is unchanged) and **absent** from `session/list` —
/// never a `"cwd": ""` row on the wire, which is what `ACP-206`'s verify pins.
fn to_acp_row(info: &StoredInfo) -> Option<AcpSessionInfo> {
    let cwd = Path::new(&info.cwd);
    if info.cwd.is_empty() || !cwd.is_absolute() {
        return None;
    }
    Some(
        AcpSessionInfo::new(SessionId::new(info.id.as_str()), cwd.to_path_buf())
            .title(title_of(info))
            .updated_at(iso8601_millis(info.modified)),
    )
}

/// The page offset an opaque cursor names (`ACP-208`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s
/// `const offset = params.cursor ? Number.parseInt(params.cursor, 10) : 0` plus
/// `Number.isFinite(offset) && offset > 0 ? offset : 0`. Three inputs are pinned by the unit's own
/// verify and all three mean "start at the beginning, never an error": `"abc"` (`NaN`), `"-5"`
/// (negative) and `"0"`.
///
/// # [CYRUP-DELTA] — `parseInt`'s leniency is reproduced, not `str::parse`'s strictness
///
/// **What differs.** `"50abc"` is `50` to `Number.parseInt` and an `Err` to `str::parse::<usize>`,
/// which would turn a page-2 cursor with a stray suffix into a silent re-send of page 1. The
/// leading-digit scan here is `parseInt`'s rule: optional sign, then digits, stop at the first
/// non-digit, empty means `NaN`.
///
/// **What it costs.** Nothing on any cursor this agent emits — they are `usize::to_string()`. It
/// costs one function that must not be "simplified" back to `parse()`, which is why it has a test
/// naming the three inputs.
fn parse_cursor(cursor: Option<&str>) -> usize {
    let Some(raw) = cursor else {
        return 0;
    };
    let trimmed = raw.trim_start();
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    let head = digits.get(..end).unwrap_or_default();
    if head.is_empty() || negative {
        // `NaN` and every negative offset are upstream's "start at the beginning".
        return 0;
    }
    // A cursor larger than `usize` is past the end of any listing: an empty page and no
    // `nextCursor`, which is the same answer JS gives for an offset beyond `filtered.length`.
    head.parse::<usize>().unwrap_or(usize::MAX)
}

/// The `text` of every text block, joined with **no separator**.
///
/// Port of pi-acp v0.0.33 `translate/pi-messages.ts`'s `normalizePiMessageText` /
/// `normalizePiAssistantText` and of `translate/bash.ts`'s `bashResultText`, over cyrup's typed
/// `Vec<Content>` instead of an `unknown`. The three upstream functions differ only in what they
/// accept as input — a bare string, an array, an array — and cyrup's content is always the array,
/// so they are one function here.
///
/// The empty-string result is load-bearing in two places: an empty user/assistant message emits
/// **no** chunk (`ACP-214`), and an empty bash result emits `terminal_exit` with no
/// `terminal_output` (`ACP-216`).
fn joined_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The tool-result envelope [`crate::translate::tool_result_to_text`] and
/// [`crate::translate::bash_exit_code`] read.
///
/// # [CYRUP-DELTA] — the envelope is rebuilt, not replayed
///
/// **What differs.** On the live path the `Value` those two functions receive is
/// `cyrup_agent`'s `result_value_of` output, carried on
/// `AgentSessionEvent::ToolExecutionEnd { result, .. }`. It is `pub(super)` there and the event is
/// long gone by replay time, so this rebuilds the two keys those functions actually read —
/// `content` and `details` — from the persisted `Message::ToolResult`.
///
/// **What it costs.** `usage`, `addedToolNames` and `terminate` are not reconstructed. Neither
/// function reads them, and `rawOutput` on a replayed row is therefore a strict subset of the live
/// one. A client diffing the two would see the difference; nothing renders it.
fn tool_result_envelope(content: &[Content], details: Option<&Value>) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "content".to_string(),
        serde_json::to_value(content).unwrap_or(Value::Null),
    );
    if let Some(details) = details {
        obj.insert("details".to_string(), details.clone());
    }
    Value::Object(obj)
}

/// The two `session/update`s one persisted tool result replays as (`ACP-215`, `ACP-216`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `loadSession` `toolResult` branch — both of them, the
/// `isBash` arm and the generic arm.
///
/// # [CYRUP-DELTA] — three of upstream's four fallbacks are unreachable, and the flicker is gone
///
/// **What differs.** (1) `toolCallId` is the **persisted** `ToolCallId`, never a fresh
/// `crypto.randomUUID()`; that is what makes the id stable across two loads and equal to the live
/// one, which `ACP-215`'s verify requires and upstream cannot satisfy. (2) `toolName` is the
/// persisted name, never the literal `'tool'`. (3) The `kind` mapping is [`ToolClass::of`] —
/// `ACP-151`'s classifier, ten ACP kinds — not upstream's three-name `read`/`edit`/other ladder.
/// **`ACP-Q34`, decided: the richer mapping**, because the unit's own text requires replay and live
/// to agree and a second classifier here is exactly how they would drift; the cost is that a
/// replayed `grep` renders as `ToolKind::Search` where pi-acp rendered `other`. (4) The first
/// update announces `in_progress`, not upstream's unconditional `status:'completed'` that the
/// second update may then downgrade to `failed` — a two-step upstream's own comment describes as a
/// client-visible flicker. [`ToolCallLedger`] makes `completed`/`failed` reachable **only** through
/// `finish`/`terminal_finish` (`ACP-129`), so the flicker is unrepresentable rather than avoided.
///
/// **What it costs.** A client that renders the first notification of a pair before the second
/// arrives shows a spinner for one frame instead of a completed row. Both arrive in the same
/// replay burst, ahead of the response (`ACP-217`).
///
/// `rawInput` is omitted rather than sent as `null`: `ToolCall` is `#[skip_serializing_none]` in
/// schema 1.7.0, so upstream's explicit `rawInput: null` is not expressible, and an absent key and
/// a null one are the same absence to every client that reads it.
fn replay_tool_result(
    ledger: &mut ToolCallLedger,
    id: &AcpToolCallId,
    tool_name: &str,
    content: &[Content],
    is_error: bool,
    details: Option<&Value>,
) -> Vec<SessionUpdate> {
    let class = ToolClass::of(tool_name);
    let mut out = Vec::with_capacity(2);
    out.push(ledger.announce(Announce {
        id: id.clone(),
        class,
        // Upstream's `bashCommand(m) ?? toolName`. A persisted `ToolResult` carries no arguments —
        // `Message::ToolResult` has `tool_name`, `content`, `is_error` and `details` and no
        // `rawInput` — so the command is not recoverable at replay time and the tool name is the
        // title for every class. Upstream reaches the same value whenever its twelve-key
        // `bashCommand` probe misses, which on a `toolResult` message it always does.
        title: tool_name.to_string(),
        status: ToolStatus::InProgress,
        locations: Vec::new(),
        raw_input: None,
        snapshot: None,
    }));

    let text = joined_text(content);
    let envelope = tool_result_envelope(content, details);
    let finish = if class.is_terminal() {
        // `bashResultText` has no JSON fallback, deliberately: an empty result must stay empty so
        // no `terminal_output` is emitted at all. `joined_text` is that function.
        ledger.terminal_finish(id, &text, is_error, bash_exit_code(&envelope, is_error))
    } else {
        // The generic arm uses the full `toolResultToText` ladder (`ACP-136`), whose first rung is
        // `details.diff` — so a replayed `edit` shows its diff exactly as the live one does.
        let rendered = crate::translate::tool_result_to_text(&envelope);
        ledger.finish(
            id,
            is_error,
            UpdatePatch {
                content: (!rendered.is_empty()).then(|| vec![ToolCallContent::from(rendered)]),
                raw_output: Some(envelope.clone()),
                ..UpdatePatch::default()
            },
        )
    };
    out.extend(finish);
    out
}

/// A session's persisted transcript as the `session/update`s `session/load` replays
/// (`ACP-214`, `ACP-215`, `ACP-216`).
///
/// Port of pi-acp v0.0.33 `agent.ts`'s `loadSession` replay loop. Pure over
/// `AgentSession::replay_items()`'s output, which is what makes every replay assertion a unit test
/// rather than an integration one.
///
/// # [CYRUP-DELTA] — cyrup's transcript has four roles ACP has no shape for
///
/// **What differs.** Upstream branches on `role === 'user' | 'assistant' | 'toolResult'` and
/// silently ignores everything else, which over pi's wire is everything else there is. cyrup's
/// `AgentMessage` is a genuine superset: `BashExecution` (a `!` shell command the user ran),
/// `Custom` (an extension's `sendMessage`), `BranchSummary` and `CompactionSummary`. Each is
/// skipped here, matching upstream's shape by arriving at it deliberately rather than by accident.
/// `ReplayItem::CacheMiss` and `ReplayItem::CompactionCost` are skipped for the same reason: they
/// are TUI chrome (`interactive-mode.ts`'s cache-miss notices) with no ACP counterpart.
///
/// **What it costs.** A session in which the user ran `!ls` replays without it, so the replayed
/// transcript is shorter than the one the TUI shows for the same file. The LLM context is
/// unaffected — this is a projection for display — and inventing a `user_message_chunk` for a
/// shell command the model never saw as a user turn would misrepresent the transcript.
#[must_use]
pub fn replay_updates(items: &[ReplayItem], cwd: &AbsCwd) -> Vec<SessionUpdate> {
    // A fresh ledger per replay: ids are the persisted ones, and the announce/finish pairing is
    // what `ACP-215`'s "both updates carry the same toolCallId" rests on. The cwd is the ledger's
    // because `ACP-130` resolves every emitted path against it.
    let mut ledger = ToolCallLedger::new(cwd.clone());
    let mut out = Vec::new();
    for item in items {
        let ReplayItem::Message(message) = item else {
            continue;
        };
        match message.as_ref() {
            AgentMessage::Core(Message::User { content, .. }) => {
                let text = joined_text(content);
                if !text.is_empty() {
                    out.push(SessionUpdate::UserMessageChunk(ContentChunk::new(
                        ContentBlock::from(text),
                    )));
                }
            }
            AgentMessage::Core(Message::Assistant(assistant)) => {
                let text = joined_text(&assistant.content);
                if !text.is_empty() {
                    out.push(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                        ContentBlock::from(text),
                    )));
                }
            }
            AgentMessage::Core(Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                details,
                ..
            }) => out.extend(replay_tool_result(
                &mut ledger,
                &AcpToolCallId::new(tool_call_id.as_str()),
                tool_name,
                content,
                *is_error,
                details.as_ref(),
            )),
            _ => {}
        }
    }
    out
}

/// Unlink a partially-built session file, having first proven it is one (`ACP-220`).
///
/// Split out of [`SessionManager::cleanup_failed_new_session`] because the containment check and
/// the unlink are the whole mechanism and are testable with nothing but a directory, while the
/// dispose that must precede them needs a live runtime. Returns whether the file is gone
/// afterwards, which includes the case where it was never there.
///
/// A path that does not resolve into the sessions root is **not** deleted and is logged: the only
/// way to reach here is `AgentSession::session_file`, so a failure means the session was built
/// against a directory this connection was not configured for, and unlinking it would be the
/// arbitrary-write's mirror image.
fn purge_partial_session_file(root: &SessionsRoot, path: &Path) -> bool {
    let file = match SessionFile::resolve(root, path) {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "ACP-220: refusing to remove a partial session file outside the sessions root"
            );
            return false;
        }
    };
    // A direct `remove_file`, NOT the trash-first `delete_session_file_at` — see
    // `SessionManager::cleanup_failed_new_session` for why adapter garbage must not reach the
    // user's trash. `NotFound` is success: the file may never have materialised.
    match std::fs::remove_file(file.path()) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            tracing::warn!(
                path = %file.path().display(),
                %error,
                "ACP-220: could not remove the partial session file; it will appear in session/list"
            );
            false
        }
    }
}

/// `ACP-209`'s critical section, isolated from the runtime so it can be tested without one.
///
/// # The bug this type exists to make unrepresentable
///
/// Upstream's `restoreSession` performs three checks with **no intervening `await`** — live
/// session, in-flight promise, else construct the async IIFE, store it in `restoringSessions`, and
/// only *then* await, with a `finally` that deletes. Because the map insert happens before the
/// first `await`, two prompts arriving in one JavaScript tick share one child: the single-threaded
/// turn boundary **is** the lock.
///
/// **That does not survive translation.** `AgentSessionRuntime::switch_session_with` takes `&self`,
/// reads `self.session().await`, drops the guard, awaits `factory.build(...)`, and only afterwards
/// calls `install` — three separate acquisitions of an `RwLock` across a build, with a
/// last-writer-wins slot replacement at the end. Nothing serializes that window. Zed sends two
/// `session/prompt`s for the same not-currently-live sessionId within one tick (window restore plus
/// a queued prompt is the concrete case); both pass the live check, both build against the **same
/// JSONL**, and the second `install` disposes the first session *after* its user message has been
/// appended and its prompt accepted. The first request never gets a response, its turn is lost, and
/// two `AgentSession`s each hold their own append fd over one file. Nothing errors. `ACP-209` is
/// `critical` for that reason and only that reason: it is silent wrong output on a normal path.
///
/// # `ACP-Q33`, decided — the guard lives here, in the ACP host
///
/// The alternative is to serialize inside `switch_session_with`, which fixes `cyrup-tui`'s
/// `/resume` and every other caller at once. It is refused **from this module**, not on the
/// merits: it changes the concurrency contract of a shared type this crate does not own, and the
/// TUI's re-entrancy (a `/resume` issued from inside a running turn) would have to be re-checked
/// against a lock that is now held across a build. The cost of deciding it here is stated plainly:
/// **the same race stays open for every non-ACP caller of `switch_session`**, and closing it there
/// is filed as this module's interface change.
///
/// # `ACP-225`, decided — the two entry points are different functions
///
/// `ACP-209` prescribes a critical section that begins with the live check re-taken inside the
/// lock; `ACP-212` requires `session/load` on the already-live id to still produce exactly one
/// `factory.build` and one `SessionReplaced`. Both cannot hold of one function, which is what
/// `ACP-225` says. The rule, taken once and asserted both ways:
///
/// * **`session/prompt` short-circuits on live** — [`RestoreGate::enter`].
/// * **`session/load` bypasses the short-circuit** — [`RestoreGate::rebuild`], which still takes
///   the same lock, so a load and a concurrent prompt-restore cannot interleave their builds.
#[derive(Default)]
pub struct RestoreGate {
    gate: tokio::sync::Mutex<()>,
}

impl RestoreGate {
    /// Return the live value if there is one, else build exactly once for all concurrent callers.
    ///
    /// `live` is called twice on purpose: once before the lock, so the common case costs nothing,
    /// and once **inside** it, which is what makes the second of two racing callers observe the
    /// first's install instead of starting its own build.
    ///
    /// # Errors
    ///
    /// Whatever `build` returns. A failed build is not cached — the gate holds no map of in-flight
    /// promises, so the next caller retries, which is upstream's `finally { delete }` semantics
    /// without the map.
    pub async fn enter<T, C, CF, B, BF, E>(&self, live: C, build: B) -> Result<T, E>
    where
        C: Fn() -> CF,
        CF: std::future::Future<Output = Option<T>>,
        B: FnOnce() -> BF,
        BF: std::future::Future<Output = Result<T, E>>,
    {
        if let Some(existing) = live().await {
            return Ok(existing);
        }
        let _guard = self.gate.lock().await;
        if let Some(existing) = live().await {
            return Ok(existing);
        }
        build().await
    }

    /// Build unconditionally, under the same lock (`ACP-212`, `ACP-225`).
    ///
    /// # Errors
    ///
    /// Whatever `build` returns.
    pub async fn rebuild<T, B, BF, E>(&self, build: B) -> Result<T, E>
    where
        B: FnOnce() -> BF,
        BF: std::future::Future<Output = Result<T, E>>,
    {
        let _guard = self.gate.lock().await;
        build().await
    }
}

// ---------------------------------------------------------------------------------------------
// The per-session plumbing: the client half, the sinks, the agent, the config pump
// ---------------------------------------------------------------------------------------------

/// The client half of one connection, or a stand-in for a manager that has none yet.
///
/// [`SessionManager`] is constructed by `AcpConnection::new`, which runs before `connect_to`, and
/// its unit tests drive `install`/`new_session` with no connection at all. `Detached` is what those
/// see: every notification is dropped and every dialog is refused, so the deny defaults
/// ([`crate::permission::deny_default`]) apply — which is the same fail-closed answer a client that
/// has hung up produces, and is therefore an honest stand-in rather than a stub.
enum WireClient {
    /// A real peer.
    Live(ConnectionTo<Client>),
    /// No connection has been attached. See the type's doc.
    Detached,
}

impl TurnSink for WireClient {
    fn notify(&self, session_id: &SessionId, update: SessionUpdate) {
        match self {
            // `ACP-122` — a `send_notification` that fails must not stop the turn completing.
            Self::Live(cx) => cx.notify(session_id, update),
            Self::Detached => {}
        }
    }
}

impl DialogClient for WireClient {
    fn request_permission(
        &self,
        request: agent_client_protocol::schema::v1::RequestPermissionRequest,
    ) -> BoxFuture<
        'static,
        Result<
            agent_client_protocol::schema::v1::RequestPermissionResponse,
            agent_client_protocol::Error,
        >,
    > {
        match self {
            Self::Live(cx) => cx.request_permission(request),
            // `ACP-150` — a refusal is answered with the deny default by the bridge, never dropped.
            Self::Detached => Box::pin(async { Err(detached_client()) }),
        }
    }

    fn create_elicitation(
        &self,
        request: agent_client_protocol::schema::v1::CreateElicitationRequest,
    ) -> BoxFuture<
        'static,
        Result<
            agent_client_protocol::schema::v1::CreateElicitationResponse,
            agent_client_protocol::Error,
        >,
    > {
        match self {
            Self::Live(cx) => cx.create_elicitation(request),
            Self::Detached => Box::pin(async { Err(detached_client()) }),
        }
    }
}

fn detached_client() -> agent_client_protocol::Error {
    AcpError::Detached("this ACP connection has no client attached".into()).into()
}

/// [`TurnSink`] over a shared [`WireClient`], because `TurnActor` takes a `Box<dyn TurnSink>` while
/// the dialog bridge takes an `Arc<C: DialogClient>` and both must be the same peer.
struct WireSink(Arc<WireClient>);

impl TurnSink for WireSink {
    fn notify(&self, session_id: &SessionId, update: SessionUpdate) {
        self.0.notify(session_id, update);
    }
}

/// The two sinks a live session must carry for the dialog seam to work at all (`ACP-144`,
/// `ACP-148`).
///
/// Held together because installing one without the other is a silent half-failure: `set_ui_sink`
/// alone leaves `UiEffect::Notify` unrouted, and `set_ui_effect_sink` alone leaves every dialog
/// falling through to `LiveHostServices`' no-sink deny defaults.
#[derive(Clone)]
struct SessionSinks {
    ui: UiSink,
    effect: UiEffectSink,
}

impl SessionSinks {
    /// Install both onto `session`'s host services — `cyrup_modes::rpc`'s `rebind_session`, minus
    /// the error listener (ACP has no `extension_error` projection; a guest error reaches the
    /// client as the failing tool call's own result).
    fn install(&self, session: &AgentSession) {
        session
            .services()
            .host_services
            .set_ui_sink(self.ui.clone());
        session
            .services()
            .host_services
            .set_ui_effect_sink(self.effect.clone());
    }
}

/// The production [`TurnAgent`]: [`RuntimeAgent`] plus the sink re-installation `ACP-061` needs.
///
/// [`RuntimeAgent::rebound`] is correctly a no-op for the *agent* — it re-reads
/// `AgentSessionRuntime::session()` on every call, so there is no cached handle to refresh — but it
/// is **not** a no-op for the sinks: a replacement brings a fresh `LiveHostServices`, whose ui slots
/// are unset, so a guest dialog on the replacement session would reach the deny default with no
/// error anywhere. This wrapper is the "that owner implements this" the trait method's doc names.
struct AcpTurnAgent {
    inner: RuntimeAgent,
    runtime: Arc<AgentSessionRuntime>,
    sinks: SessionSinks,
}

impl AcpTurnAgent {
    fn new(runtime: Arc<AgentSessionRuntime>, sinks: SessionSinks) -> Self {
        Self {
            inner: RuntimeAgent::new(Arc::clone(&runtime)),
            runtime,
            sinks,
        }
    }
}

impl TurnAgent for AcpTurnAgent {
    fn start_run<'a>(
        &'a self,
        input: UserInput,
    ) -> BoxFuture<'a, Result<RunStarted, SessionServiceError>> {
        self.inner.start_run(input)
    }

    fn fold_into_run<'a>(
        &'a self,
        input: UserInput,
    ) -> BoxFuture<'a, Result<cyrup_session_svc::PromptAccepted, SessionServiceError>> {
        self.inner.fold_into_run(input)
    }

    fn abort<'a>(&'a self) -> BoxFuture<'a, ()> {
        self.inner.abort()
    }

    fn snapshot<'a>(&'a self, abs: PathBuf, named: PathBuf) -> BoxFuture<'a, FileSnapshot> {
        self.inner.snapshot(abs, named)
    }

    /// `ACP-061` / `ACP-154` — re-install the ui sinks on whatever session is live now.
    fn rebound<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let session = self.runtime.session().await;
            self.sinks.install(&session);
        })
    }
}

/// The session-wide pump: the **single emitter** of `config_option_update`, `current_mode_update`
/// and `session_info_update` (`ACP-077`, `ACP-285`, `ACP-Q20`).
///
/// Port of the intent of pi-acp v0.0.33 `session.ts`'s absent handling of `model_changed` —
/// upstream has no arm for it at all, which is the latent defect `ACP-077` identifies: a model
/// changed by anything other than `session/set_config_option` leaves the client's dropdown stale
/// with no error anywhere.
///
/// # Why this is a second stream, and why that does not violate `ACP-153`
///
/// `ACP-153` governs where a **turn** learns it has settled, and its rule is that the turn must
/// read the *run-scoped* stream: a session-wide subscription would settle the ACP turn on an
/// `AgentSettled` belonging to a run this turn did not start (an extension's own `ctx.prompt`).
/// This pump settles nothing. It reads three events that describe the **session** rather than any
/// run, two of which are emitted while no run is active at all — where a run-scoped stream does not
/// exist — so it is the only place they can be observed.
///
/// # `ACP-Q20`, honoured
///
/// The setters ([`SessionManager::set_mode`], [`SessionManager::set_config_option`],
/// `crate::commands`' `/name` arm) perform their mutation and emit nothing. Every notification for
/// these three facts is written here, so an ACP client, the TUI and an extension all produce
/// exactly one update per change, by the same route.
///
/// # The re-subscription, and why it cannot spin
///
/// `AgentSessionRuntime::watch_generation` bumps on every replacement, so the pump re-subscribes to
/// the new session's fanout rather than going quiet after an extension's `ctx.newSession()`. The
/// loop exits — never spins — on two conditions: the watch channel closing (the runtime is gone) and
/// the event stream ending with no generation bump (this session's fanout is closed for good).
///
/// # `ACP-061` — this is also where the ui sinks are re-installed
///
/// [`TurnAgent::rebound`] covers a replacement that happens **during** a turn: the turn observes
/// `SessionReplaced` on its run-scoped stream and calls it. A replacement while the turn is IDLE
/// produces no such event on any stream the turn holds, so the generation watch is the only
/// observer of it — and a fresh session brings a fresh `LiveHostServices` whose ui slots are unset,
/// which would leave every guest dialog on the replacement session falling through to the no-sink
/// deny default with nothing anywhere saying why. Re-installing here makes both routes cover it,
/// and installing twice is idempotent (the slots are set, not appended to).
async fn config_pump(
    session_id: SessionId,
    runtime: Arc<AgentSessionRuntime>,
    wire: Arc<WireClient>,
    sinks: SessionSinks,
    rename_echo: crate::commands::RenameEcho,
) {
    let mut generation = runtime.watch_generation();
    loop {
        let session = runtime.session().await;
        // `ACP-061` — before the first event of this generation is read, so a dialog opened by a
        // `session_start` handler on the replacement already has somewhere to go.
        sinks.install(&session);
        let mut events = session.subscribe();
        loop {
            let event = tokio::select! {
                // Biased so a replacement is observed before the dying stream's tail: the tail's
                // updates describe a session the client is about to be told is gone.
                biased;
                changed = generation.changed() => {
                    if changed.is_err() {
                        // The runtime was dropped. Nothing left to describe.
                        return;
                    }
                    break;
                }
                event = events.next() => match event {
                    Some(event) => event,
                    // The fanout closed with no replacement behind it.
                    None => return,
                },
            };
            for update in session_updates_for(&session, &event, &rename_echo).await {
                wire.notify(&session_id, update);
            }
        }
    }
}

/// The three arms of [`config_pump`], split out so they are table-testable without a runtime.
///
/// Everything else is deliberately dropped: this pump is **not** a second translator, and an arm
/// added here for an event `crate::translate` already handles would double it on the wire.
async fn session_updates_for(
    session: &AgentSession,
    event: &AgentSessionEvent,
    rename_echo: &crate::commands::RenameEcho,
) -> Vec<SessionUpdate> {
    match event {
        // `ACP-077` — the whole option set is re-derived, never patched, so the advertised
        // `currentValue` is the session's state read back independently.
        AgentSessionEvent::ModelChanged { .. } => {
            vec![crate::config_options::config_options_update(session).await]
        }
        // `ACP-072` / `ACP-077` — both, in this order: `current_mode_update` is what moves the
        // client's mode selector, and the config option carries the same fact for a client that
        // renders the `thought_level` dropdown instead.
        AgentSessionEvent::ThinkingLevelChanged { level } => {
            let mut out = Vec::with_capacity(2);
            if let Some(applied) = crate::config_options::thinking_level_from_id(level) {
                out.push(crate::config_options::current_mode_update(applied));
            } else {
                // The session emitted a level this adapter has no id for, which can only mean
                // `ModelThinkingLevel` grew a rung. Emitting a `current_mode_update` for a mode the
                // client was never offered would leave its selector on a value it cannot render, so
                // only the re-derived option set is sent — which will carry the new rung.
                tracing::warn!(
                    level,
                    "ACP-062: a thinking level with no advertised mode id"
                );
            }
            out.push(crate::config_options::config_options_update(session).await);
            out
        }
        // `ACP-285` — a rename from any route (`/name`, an extension, another front-end).
        //
        // `ACP-122` — unless the route that caused it already emitted the update in its own
        // ordered output. That is only the `/name` built-in, which is answered above the turn
        // queue and would otherwise always lose the race to this task; see
        // [`crate::commands::RenameEcho`].
        AgentSessionEvent::SessionInfoChanged { name } => rename_update(name.clone(), rename_echo),
        _ => Vec::new(),
    }
}

/// The `SessionInfoChanged` arm of [`session_updates_for`], split out because it is the only one
/// with a decision in it and the only one testable without an `AgentSession` (`ACP-122`).
///
/// Returns nothing when the route that caused the rename already emitted the update in its own
/// ordered output; see [`crate::commands::RenameEcho`].
fn rename_update(
    name: Option<String>,
    rename_echo: &crate::commands::RenameEcho,
) -> Vec<SessionUpdate> {
    if rename_echo.take() {
        Vec::new()
    } else {
        vec![crate::config_options::session_info_update(
            name,
            now_iso8601_millis(),
        )]
    }
}

/// The registry pi-acp holds as a `Map<string, PiAcpSession>`, collapsed to one slot (`ACP-120`).
///
/// `SessionId(Arc<str>)` is cheap to clone as the key, so the slot holds the id alongside the
/// runtime rather than deriving it.
pub struct SessionManager {
    host: Arc<dyn AcpHost>,
    /// The one live session, if any. `tokio::sync::Mutex` rather than `std::sync::Mutex` because
    /// every transition here awaits (`build_runtime`, `dispose`).
    live: tokio::sync::Mutex<Option<LiveSession>>,
    /// `ACP-082` / `ACP-207` — connection-scoped state that `session/new` and `session/load` WRITE
    /// and `session/list` READS as its default cwd filter. Upstream's `lastSessionCwd`.
    last_cwd: tokio::sync::Mutex<Option<AbsCwd>>,
    /// `ACP-209` / `ACP-225`. See [`RestoreGate`].
    gate: RestoreGate,
    /// The connection, recorded by the first handler that holds one ([`SessionManager::attach`]).
    ///
    /// The manager is constructed by `AcpConnection::new`, which runs **before** `connect_to`, so
    /// there is no connection to hand it at construction; every handler has one. `OnceLock` rather
    /// than a field because a connection serves exactly one peer for its whole life.
    cx: OnceLock<ConnectionTo<Client>>,
    /// What `initialize` recorded, shared with the owning [`crate::connection::AcpConnection`].
    /// Read for [`DialogCaps`] (`ACP-147`) when a session's dialog bridge is spawned.
    client: Arc<OnceLock<ClientView>>,
    /// The per-session plumbing that must be reachable from a **synchronous** handler:
    /// `session/cancel` runs inside the dispatch loop and may not await (`ACP-123`).
    ///
    /// `std::sync::Mutex`, and the guard is never held across an `.await` — every use is a clone
    /// of the cheap [`TurnHandle`] or a `replace`.
    bound: Mutex<Option<SessionBinding>>,
}

/// The per-session plumbing [`SessionManager::install`] spawns and [`SessionManager::install`]
/// replaces: the turn actor, the dialog bridge and the session-wide config pump.
struct SessionBinding {
    /// The id this binding serves. `session/cancel` for any other id is a no-op.
    session_id: SessionId,
    /// `ACP-121` / `ACP-153`. Cheap to clone; the actor owns the `Turn`.
    turn: TurnHandle,
    /// `ACP-077` / `ACP-285`. Aborted when the binding is replaced — its stream would otherwise
    /// outlive the session it describes.
    pump: tokio::task::JoinHandle<()>,
    /// `ACP-122` / `ACP-285`. Shared with this binding's `pump`; see
    /// [`crate::commands::RenameEcho`]. Per-binding rather than per-manager so a claim left behind
    /// by a replaced session cannot silence a rename on its successor.
    rename_echo: crate::commands::RenameEcho,
}

/// The live half of the slot.
pub struct LiveSession {
    /// The id this connection told the client.
    pub id: SessionId,
    /// The runtime host. `session/new`, `session/load` and the `session/prompt` restore path all
    /// replace it; `close` disposes it.
    pub runtime: Arc<AgentSessionRuntime>,
    /// The cwd it was built for, already checked (`ACP-056`).
    pub cwd: AbsCwd,
}

impl LiveSession {
    /// Dispose this session and hand back the path of the file it was writing (`ACP-219`).
    ///
    /// # Why this consumes `self`, and why the path comes out of the same call
    ///
    /// `ACP-219` is `critical` for one reason: unlinking the live session's file **without
    /// disposing it first** leaves the session running with `DiskStore`'s `O_APPEND` fd open on an
    /// unlinked inode. Every subsequent turn is written to a file no listing, no `session/load`
    /// and no user can reach, and *nothing errors* — the client sees a healthy session. That is
    /// silent data loss on a normal path (a session picker with a delete affordance, used on the
    /// row that is currently open).
    ///
    /// A `dispose().await` on the line above the unlink is a statement a refactor can move,
    /// reorder or delete, and no test in the tree can observe its absence: the failure it prevents
    /// needs a real `DiskStore` holding a real fd, and `AgentSessionRuntime` has no constructor
    /// short of a provider-backed build. So the ordering is made a property of the type instead —
    /// **the path is only obtainable by consuming the session, and consuming it disposes it.**
    /// `SessionManager::delete_session` has no other way to learn where to unlink, and the one
    /// remaining place the two statements sit together is this method, whose name says what it
    /// does.
    ///
    /// `None` when the session was never persisted (`SessionConfig.persist == false`, or a session
    /// that never reached its first assistant message). The dispose still happens.
    ///
    /// **What disposing costs (`ACP-224`).** `dispose` opens with `abort_and_settle()` and fires
    /// `session_cancel`, which kills tracked bash children — so a client that deletes a session it
    /// is mid-turn in loses that turn, where pi-acp would have completed it. That is stated as one
    /// rule with `delete_session`'s doc so the two cannot disagree.
    pub async fn dispose_and_take_path(self) -> Option<PathBuf> {
        let path = self.runtime.session().await.session_file().await;
        self.runtime.dispose().await;
        path
    }
}

/// What `session/load` produces, in the three parts `ACP-217` requires be written in **this** order.
///
/// # Why this is not [`crate::HandlerOutcome`]
///
/// `HandlerOutcome` models "respond, then notify", which is the ordering `session/new` needs
/// (`ACP-068`/`ACP-069`) and which [`crate::respond_then_notify`] enforces. `session/load` needs
/// **both** directions at once and upstream is explicit about why: every replay update is `await`ed
/// inside `loadSession` so the client has the transcript before the response settles, while the
/// `available_commands_update` is deliberately deferred with `setTimeout(fn, 0)` because *"some
/// clients (e.g. Zed) will ignore notifications for an unknown sessionId"*.
///
/// `LoadSessionResponse` also names no session — the id was the client's, in the request — so
/// `SessionScoped` returns `None` for it and `respond_then_notify` has nothing to address a
/// follow-up to. That is not an oversight in `lib.rs`; it is why `session/load` gets its own
/// outcome type and its own driver, [`SessionManager::handle_load`].
pub struct LoadOutcome {
    /// The session these updates belong to — the client's own id, echoed back.
    pub session_id: SessionId,
    /// Written **before** the response, in order (`ACP-214`…`ACP-217`).
    pub replay: Vec<SessionUpdate>,
    /// The response itself.
    pub response: LoadSessionResponse,
    /// Written **after** the response (`ACP-217`).
    pub follow_up: Vec<SessionUpdate>,
}

impl SessionManager {
    /// A manager with nothing live yet.
    #[must_use]
    pub fn new(host: Arc<dyn AcpHost>) -> Self {
        Self {
            host,
            live: tokio::sync::Mutex::new(None),
            last_cwd: tokio::sync::Mutex::new(None),
            gate: RestoreGate::default(),
            cx: OnceLock::new(),
            client: Arc::new(OnceLock::new()),
            bound: Mutex::new(None),
        }
    }

    /// The cell `initialize` writes its [`ClientView`] into.
    ///
    /// Shared with the owning [`crate::connection::AcpConnection`] rather than duplicated: the
    /// dialog bridge's [`DialogCaps`] (`ACP-147`) and the connection's own `client()` accessor must
    /// be the same value, and two cells is how they come to disagree.
    #[must_use]
    pub fn client_cell(&self) -> Arc<OnceLock<ClientView>> {
        Arc::clone(&self.client)
    }

    /// Record the connection this manager answers on. Idempotent; the first wins.
    ///
    /// Every handler calls this before doing anything else, because the manager outlives no
    /// connection and is built before one exists. Without it a session installed by `session/new`
    /// would have nowhere to send `session/update` — the turn's notifications, the dialog bridge's
    /// permission requests and the config pump all ride this.
    pub fn attach(&self, cx: &ConnectionTo<Client>) {
        let _ = self.cx.set(cx.clone());
    }

    /// The client half of this connection, or the detached stand-in.
    fn wire(&self) -> Arc<WireClient> {
        Arc::new(match self.cx.get() {
            Some(cx) => WireClient::Live(cx.clone()),
            None => WireClient::Detached,
        })
    }

    /// The host this manager builds through.
    #[must_use]
    pub fn host(&self) -> &Arc<dyn AcpHost> {
        &self.host
    }

    /// The live turn handle, when its id matches. `ACP-120`'s gate for the cancel path.
    fn turn_for(&self, id: &SessionId) -> Option<TurnHandle> {
        let bound = self.bound.lock().ok()?;
        let binding = bound.as_ref()?;
        (binding.session_id == *id).then(|| binding.turn.clone())
    }

    /// This session's rename echo, shared with its `config_pump` (`ACP-122`).
    ///
    /// A miss yields a fresh, unshared echo rather than `None`: the only caller is the `/name`
    /// built-in, whose session has just been resolved, and an echo nobody reads simply means the
    /// pump emits the update on its own — the pre-fix behaviour, which is wrong about ordering but
    /// never wrong about content. Failing the command instead would be worse.
    fn rename_echo_for(&self, id: &SessionId) -> crate::commands::RenameEcho {
        self.bound
            .lock()
            .ok()
            .and_then(|bound| {
                let binding = bound.as_ref()?;
                (binding.session_id == *id).then(|| binding.rename_echo.clone())
            })
            .unwrap_or_default()
    }

    /// `Unknown sessionId: <id>` — the lookup miss, byte-for-byte from pi-acp v0.0.33
    /// `session.ts`'s `SessionManager.get` (`ACP-120`, `ACP-078`, `ACP-210`).
    ///
    /// Built by hand with `Error::new(-32602, ..)` and NOT through `From<ErrorCode>`, which would
    /// stamp strum's `"Invalid params"` over the message. This is user-visible in Zed.
    #[must_use]
    pub fn unknown_session(id: &SessionId) -> AcpFailure {
        AcpFailure::InvalidParams {
            message: format!("Unknown sessionId: {id}"),
        }
    }

    /// The live session, if its id matches. `None` for a miss — the caller decides whether that is
    /// [`SessionManager::unknown_session`] or a restore.
    pub async fn get(&self, id: &SessionId) -> Option<Arc<AgentSessionRuntime>> {
        let live = self.live.lock().await;
        live.as_ref()
            .filter(|s| s.id == *id)
            .map(|s| Arc::clone(&s.runtime))
    }

    /// Remove the live session from the slot **without disposing it**, if its id matches.
    ///
    /// The half of `session/delete` that `ACP-219` turns on: the caller must dispose the returned
    /// runtime before the file is unlinked, and taking it out of the slot first means no other
    /// handler can hand it to a prompt in between. Deliberately not `pub` beyond this crate's own
    /// entry points — a caller that takes the session and forgets to dispose it leaks a live agent.
    async fn take_live(&self, id: &SessionId) -> Option<LiveSession> {
        let mut live = self.live.lock().await;
        if live.as_ref().is_some_and(|s| s.id == *id) {
            live.take()
        } else {
            None
        }
    }

    /// Upstream's `lastSessionCwd` read (`ACP-082`, `ACP-207`).
    pub async fn last_cwd(&self) -> Option<AbsCwd> {
        self.last_cwd.lock().await.clone()
    }

    /// Upstream's `lastSessionCwd` write. `session/new` and `session/load` both perform it;
    /// `ACP-226` pins that a `session/load` for an unresolvable id must **not**, so this is called
    /// after validation rather than before it.
    pub async fn set_last_cwd(&self, cwd: AbsCwd) {
        *self.last_cwd.lock().await = Some(cwd);
    }

    /// Install a freshly built runtime, disposing whatever was live (`ACP-061`, `ACP-212`).
    ///
    /// Upstream's `closeAllExcept(keep)`; here the eviction is structural. `runtime_ready` is
    /// invoked on the NEW runtime before the slot is published, so `ACP-023`'s signal watcher is
    /// armed before anything can be dispatched against it.
    /// The three tasks a live session needs, spawned together so a half-installed session cannot
    /// exist.
    ///
    /// The order inside is the one thing that matters and it is: **install the sinks on the session
    /// before anything can run against it.** A guest that opens a dialog before `set_ui_sink` has
    /// been called reaches `LiveHostServices`' no-sink deny default — fail-closed, but not the
    /// feature — and there is no second chance, because the guest is already blocked in
    /// `ui_roundtrip`.
    ///
    /// `ACP-155`'s task split is realised here and nowhere else: the turn actor owns the pump, the
    /// notifications and the responder; the dialog bridge owns its own channel and detaches one task
    /// per dialog; the config pump owns the session-wide stream. Three tasks, no shared lock, and
    /// the only one that can block on a human is the one with nothing else to do.
    async fn bind(&self, live: &LiveSession) -> SessionBinding {
        let session = live.runtime.session().await;
        let wire = self.wire();

        // --- the dialog bridge (`ACP-144`…`ACP-150`) ------------------------------------------
        let bridge = PermissionBridge::new();
        let sinks = SessionSinks {
            ui: bridge.sink(),
            effect: bridge.effect_sink(),
        };
        sinks.install(&session);
        let caps = self
            .client
            .get()
            .map_or_else(DialogCaps::default, DialogCaps::from_client);
        tokio::spawn(bridge.run_with_caps(live.id.clone(), Arc::clone(&wire), caps));

        // --- the turn actor (`ACP-121`, `ACP-153`, `ACP-155`) ---------------------------------
        let agent = AcpTurnAgent::new(Arc::clone(&live.runtime), sinks.clone());
        let turn = crate::turn::TurnActor::spawn(
            live.id.clone(),
            live.cwd.clone(),
            Arc::new(agent),
            Box::new(WireSink(Arc::clone(&wire))),
        );

        // --- the config pump (`ACP-077`, `ACP-285`) -------------------------------------------
        let rename_echo = crate::commands::RenameEcho::default();
        let pump = tokio::spawn(config_pump(
            live.id.clone(),
            Arc::clone(&live.runtime),
            wire,
            sinks,
            rename_echo.clone(),
        ));

        SessionBinding {
            session_id: live.id.clone(),
            turn,
            pump,
            rename_echo,
        }
    }

    pub async fn install(&self, session: LiveSession) {
        self.host.runtime_ready(&session.runtime);
        let binding = self.bind(&session).await;
        let previous = self.live.lock().await.replace(session);

        // Replace the plumbing in the same breath as the slot. The OLD turn actor is shut down
        // rather than dropped, because `TurnActor::run`'s teardown is what answers an outstanding
        // `session/prompt` with `cancelled` — dropping the handle reaches the same code path, but
        // saying so is the difference between a guarantee and a coincidence.
        let previous_binding = match self.bound.lock() {
            Ok(mut bound) => bound.replace(binding),
            // A poisoned lock means a panic in a `Mutex` guard, and this crate contains no panic.
            // The new binding is still live and reachable through `bound` for every later handler;
            // losing the old one costs a shutdown message the drop of its handle also sends.
            Err(_) => None,
        };
        if let Some(previous_binding) = previous_binding {
            previous_binding.turn.shutdown();
            // The pump's stream would otherwise outlive the session it describes and go on pushing
            // `config_option_update`s for a runtime the client has been told is gone.
            previous_binding.pump.abort();
        }

        if let Some(previous) = previous {
            // `close` is `AgentSessionRuntime::dispose().await` — upstream's swallowing
            // try/catch has no counterpart, because `dispose` is infallible.
            previous.runtime.dispose().await;
        }
    }

    /// Resolve one client-supplied session id to a stored session (`ACP-201`, `ACP-202`,
    /// `ACP-210`, `ACP-291`).
    ///
    /// The one place an id from the wire becomes a path, and it does so in two steps that are both
    /// checks: [`AcpSessionId::parse`] (pi's own `assertValidSessionId`, so an id carrying `../`
    /// or an absolute-looking segment never reaches a `join`), then [`find_stored`], whose result
    /// is a [`SessionFile`] proven to lie under the sessions root.
    ///
    /// # `ACP-291` — one validator, every path
    ///
    /// A malformed id is `InvalidParams` carrying pi's own validator sentence, on **every** id-
    /// bearing path, rather than being folded into `Unknown sessionId`. The two are different
    /// facts — "you sent something that is not a session id" and "no session has that id" — and a
    /// client that cannot tell them apart cannot tell a bug in its own id handling from a stale
    /// history entry. A well-formed id with no session is [`SessionManager::unknown_session`],
    /// which is what `ACP-210`'s byte-exactness pins.
    ///
    /// # Errors
    ///
    /// [`AcpFailure::InvalidParams`] for a malformed id or for an id no session file carries.
    pub fn locate(&self, id: &SessionId) -> Result<StoredSession, AcpFailure> {
        let parsed = AcpSessionId::parse(&id.0)?;
        find_stored(&self.host.sessions_root(), &parsed).ok_or_else(|| Self::unknown_session(id))
    }

    /// Build a runtime for `stored` at `cwd` and publish it as the live session.
    ///
    /// The shared tail of `session/new`'s, `session/load`'s and `session/prompt`'s restore paths.
    /// `set_last_cwd` happens **after** a successful build, which is `ACP-226`'s requirement read
    /// forward: a failure must leave the default `session/list` filter as it was.
    ///
    /// # Errors
    ///
    /// [`AcpFailure`] from [`AcpFailure::classify`] over the host's typed `SessionServiceError`
    /// (`ACP-058`, `ACP-221`). `MissingSessionCwd` — a session whose recorded cwd was deleted —
    /// lands on `Internal`, deliberately: it is an ordinary input for a stale history entry and
    /// must reach the client as an error the connection survives, never as an auth prompt.
    async fn build_and_install(
        &self,
        id: &SessionId,
        target: SessionTarget,
        cwd: AbsCwd,
    ) -> Result<Arc<AgentSessionRuntime>, AcpFailure> {
        let request = RuntimeRequest {
            cwd: cwd.clone(),
            target,
        };
        let runtime = self
            .host
            .build_runtime(&request)
            .await
            .map_err(AcpFailure::from)?;
        self.install(LiveSession {
            id: id.clone(),
            runtime: Arc::clone(&runtime),
            cwd: cwd.clone(),
        })
        .await;
        self.set_last_cwd(cwd).await;
        Ok(runtime)
    }

    /// The `session/prompt` restore path — single-flight, short-circuiting on the live session
    /// (`ACP-201`, `ACP-202`, `ACP-209`, `ACP-221`, `ACP-225`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `restoreSession`, minus the `PiRpcProcess.spawn`
    /// try/catch: there is no child, so `PiRpcSpawnError` and its `data: {code}` shape have no
    /// counterpart and the typed `SessionServiceError` classifies instead.
    ///
    /// # Errors
    ///
    /// [`SessionManager::unknown_session`] for an id nothing on disk carries,
    /// [`AcpFailure::InvalidParams`] for a malformed one, `Internal` for a session whose header
    /// records no absolute cwd, and whatever [`AcpFailure::classify`] makes of a build failure.
    ///
    /// **The caller must map this with `responder.respond_with_error(..)` and still return
    /// `Ok(())` from its spawned task.** `ConnectionTo::spawn`'s own doc is explicit that a task
    /// returning `Err` shuts the whole server down, and a `session/prompt` for a session whose cwd
    /// has since been deleted is an ordinary input — mapping it with `?` kills the editor's agent
    /// connection (`ACP-221`).
    pub async fn restore_session(
        &self,
        id: &SessionId,
    ) -> Result<Arc<AgentSessionRuntime>, AcpFailure> {
        self.gate
            .enter(
                || self.get(id),
                || async {
                    let stored = self.locate(id)?;
                    let cwd = stored.cwd.clone().ok_or_else(|| AcpFailure::Internal {
                        message: format!(
                            "session {id} records no absolute working directory and cannot be \
                             restored"
                        ),
                    })?;
                    self.build_and_install(
                        id,
                        SessionTarget::Resume(stored.file.path().to_path_buf()),
                        cwd,
                    )
                    .await
                },
            )
            .await
    }

    /// Remove the JSONL a `session/new` created and then decided was unusable (`ACP-220`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `cleanupFailedNewSession`, whose point is that pi has
    /// already created and written a session file by the time the adapter refuses the session, so
    /// without this the file becomes a permanent ghost in every future `session/list`. The same
    /// hazard exists in-process for a different reason: the build's own `model_change` /
    /// `thinking_level_change` appends are the moment the file materialises.
    ///
    /// Order matters and is upstream's: **dispose first**, then unlink. `DiskStore` holds an
    /// `O_APPEND` fd whose doc says in terms that the pre-existing reopen-per-append behaviour
    /// *"silently recreated a session file deleted underneath a live manager — leaving a headerless
    /// stub"*, and that the held fd is what stops it. Unlinking under a live session is exactly
    /// the shape `ACP-220`'s verify forbids ("no header-only stub").
    ///
    /// # `ACP-220`'s two open decisions, taken
    ///
    /// **The trash is not used here, unlike `session/delete`.** `delete_session_file_at` is
    /// trash-first, which is right for a session a *user* asked to delete and wrong for a partial
    /// file the adapter created and the user never saw: it would put adapter garbage in the user's
    /// trash under a name they cannot connect to anything. This is a direct `remove_file`.
    ///
    /// **There is one cleanup path, not upstream's four.** Upstream calls this from three
    /// auth-shaped sites plus a generic one because `newSession` can refuse for three different
    /// credential reasons. Whether cyrup has any of those depends on `ACP-Q7` (the modelless-session
    /// rule), which is 4b's to settle — so this is called from the single post-build failure arm of
    /// [`SessionManager::new_session`] and covers whatever that arm grows.
    ///
    /// The containment check is not optional and is not defence in depth: the path comes from
    /// `AgentSession::session_file`, and [`SessionFile::resolve`] is what makes "the adapter
    /// deletes a file" true only of files under the sessions root.
    ///
    /// # This has no reachable caller today
    ///
    /// Its one call site is [`SessionManager::new_session`]'s post-build failure arm, and under
    /// `ACP-Q7`'s decision [`SessionManager::decorate_new_session`] cannot fail — see that
    /// function's doc for the full statement and for what would make this live again. The
    /// mechanism (`purge_partial_session_file`) IS tested in all four of its cases; the four lines
    /// of glue here are not, and cannot be until something above can refuse.
    pub async fn cleanup_failed_new_session(&self, runtime: &Arc<AgentSessionRuntime>) {
        let path = runtime.session().await.session_file().await;
        runtime.dispose().await;
        if let Some(path) = path {
            purge_partial_session_file(&self.host.sessions_root(), &path);
        }
    }

    /// `session/new` (`ACP-056`…`ACP-069`, `ACP-213`, `ACP-220`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `newSession`. This body owns the **session-management**
    /// half — the `isAbsolute` guard, the build, the one-slot install, the `lastSessionCwd` write
    /// and `ACP-220`'s cleanup — and hands the response decoration to
    /// [`SessionManager::decorate_new_session`], which is where 4b's `modes` / `configOptions` /
    /// `models` / startup-prelude units land.
    ///
    /// # Errors
    ///
    /// Every `AcpFailure` the client should see. Note this returns `Result`, not a panic and not a
    /// silent drop: `ACP-057`'s second assertion is that a build failure yields an error response
    /// **and** that the connection answers a later request.
    pub async fn new_session(
        &self,
        req: &NewSessionRequest,
        client: Option<&ClientView>,
    ) -> Result<HandlerOutcome<NewSessionResponse>, AcpFailure> {
        // `ACP-056` — first statement, before any filesystem work, and the only parser.
        let cwd = AbsCwd::parse(req.cwd.clone())?;
        let runtime = self
            .host
            .build_runtime(&RuntimeRequest {
                cwd: cwd.clone(),
                target: SessionTarget::New,
            })
            .await
            .map_err(AcpFailure::from)?;

        // cyrup mints the id; upstream reads it back off the child's `session_start`. The response
        // must carry the id the session file is named after, which is why it is read from the
        // session rather than generated here (ADR-0028 §5 — the filename match IS the map).
        let id = SessionId::new(runtime.session().await.session_id().as_str());

        match self
            .decorate_new_session(&runtime, client, id.clone())
            .await
        {
            Ok(outcome) => {
                self.install(LiveSession {
                    id,
                    runtime,
                    cwd: cwd.clone(),
                })
                .await;
                self.set_last_cwd(cwd).await;
                Ok(outcome)
            }
            Err(failure) => {
                // `ACP-220` — the file exists by now. Dispose, unlink, and return the ORIGINAL
                // protocol error; a cleanup failure never replaces it.
                self.cleanup_failed_new_session(&runtime).await;
                Err(failure)
            }
        }
    }

    /// The response half of `session/new`, and the one post-build failure arm `ACP-220` cleans up
    /// after.
    ///
    /// `ACP-062` / `ACP-064` / `ACP-065` / `ACP-069`. The mode list and the config options come
    /// from **one** view read ([`crate::config_options::session_surface`]) so a model change cannot
    /// straddle them; the command advertisement is `follow_up`, i.e. after the response, which is
    /// upstream's `setTimeout(fn, 0)` and its stated reason.
    ///
    /// `ACP-065`, decided: **no `models` payload and no `_meta.piAcp` shim.** The `model` config
    /// option carries the same information in the spec-sanctioned place, and a second source of
    /// truth is a second thing that can disagree.
    ///
    /// # `ACP-Q7`, decided — a session with no resolvable model is **not** refused
    ///
    /// Upstream refuses, because pi-acp cannot show a model picker and a modelless pi session can
    /// do nothing. cyrup's cannot do nothing: the `model` config option is advertised on this very
    /// response, `session/set_config_option` sets it, and `--terminal-login` (`ACP-010`) exists
    /// precisely so a credential-less first run has somewhere to go. Refusing here would make
    /// `cyrup --acp` unusable in exactly the state the auth method was added for.
    ///
    /// **What it costs.** A `session/prompt` on such a session is refused by the preflight rather
    /// than at `session/new`, so the error arrives one round trip later and reads as
    /// `No model selected` instead of as a failed session creation. `ACP-126` carries that error
    /// faithfully and the connection survives it.
    ///
    /// # `ACP-060` / `ACP-220` — the rollback arm this `Result` exists for is currently
    /// **unreachable**, and that is a consequence of `ACP-Q7`, not an oversight
    ///
    /// `ACP-220`'s verify — *"assert the file `session/new` created is gone"* — is **not
    /// satisfiable as written**, and the reason is one decision above: `ACP-Q7` says a session
    /// with no resolvable model is not refused, and every other statement in this body
    /// ([`crate::config_options::session_surface`], [`crate::startup::startup_prelude`],
    /// [`crate::commands::available_commands_update`]) is infallible. So this function returns
    /// `Ok` on every path, `SessionManager::new_session`'s `Err` arm cannot be reached, and
    /// [`SessionManager::cleanup_failed_new_session`] never runs. The `#[allow]` below is that
    /// fact, spelled.
    ///
    /// The seam is kept rather than deleted because it is one `?` away from being live and the
    /// mechanism behind it *is* tested (`a_partial_session_file_is_purged_only_from_inside_the_
    /// sessions_root`, over [`purge_partial_session_file`]). What would make it reachable, in
    /// order of likelihood: reversing `ACP-Q7`; a `session_surface` that can fail once a guest
    /// provider's catalog is fetched rather than read; or any post-build validation this handler
    /// grows. Whoever adds one should write `ACP-220`'s assertion at the same time — it is
    /// unwritable until then, and a test that cannot fail is worse than a missing one.
    ///
    /// # Errors
    ///
    /// [`AcpFailure`] from the surface read. The `Result` is also the seam `ACP-220`'s cleanup
    /// hangs from: a body that could not fail would leave no post-build failure arm.
    #[allow(clippy::unnecessary_wraps)]
    async fn decorate_new_session(
        &self,
        runtime: &Arc<AgentSessionRuntime>,
        client: Option<&ClientView>,
        id: SessionId,
    ) -> Result<HandlerOutcome<NewSessionResponse>, AcpFailure> {
        // `ACP-054` is the client's, read at `initialize`; nothing on the session response varies
        // with it, which is why this parameter is unused rather than absent — it is the seam
        // `ACP-Q16`'s startup-prelude `_meta` would have used, and `ACP-065`'s decision closes it.
        let _ = client;
        let session = runtime.session().await;
        let (modes, options) = crate::config_options::session_surface(&session).await;
        // `ACP-068` / `ACP-069` — both follow-ups, in this order, and both AFTER the response.
        // `HandlerOutcome::with_follow_up` is what makes "respond, then notify, on one task" the
        // only expressible shape; see `crate::startup`'s module doc for the ordering guarantee
        // that stands in for upstream's two `setTimeout(…, 0)`s.
        //
        // `ACP-081` — a project with nothing to report contributes no chunk at all, rather than
        // upstream's degenerate single-newline one.
        let mut follow_up = Vec::with_capacity(2);
        follow_up.extend(crate::startup::startup_prelude(&session));
        follow_up.push(crate::commands::available_commands_update(&session));
        Ok(HandlerOutcome::with_follow_up(
            NewSessionResponse::new(id)
                .modes(modes)
                .config_options(options),
            follow_up,
        ))
    }

    /// `session/load`'s pure core (`ACP-211`…`ACP-217`, `ACP-225`, `ACP-226`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `loadSession`, **with its statement order corrected**.
    ///
    /// # [CYRUP-DELTA] — validation precedes teardown, where upstream's follows it (`ACP-226`)
    ///
    /// **What differs.** Upstream calls `this.sessions.close(params.sessionId)` and writes
    /// `this.lastSessionCwd = params.cwd` **before** `findStoredSession`, so a `session/load` for
    /// an id that does not exist disposes the live session and re-scopes the default `session/list`
    /// filter to a project the client never opened — two side effects of a request that then
    /// fails. Here the cwd is parsed, the id is resolved and only then is anything replaced.
    ///
    /// **What it costs.** Nothing observable to a client sending a valid id; the two are
    /// indistinguishable on the success path.
    ///
    /// # `ACP-225` — this bypasses the live short-circuit, and that is the rule
    ///
    /// [`RestoreGate::rebuild`] takes the same lock as the prompt path and then builds
    /// unconditionally, so `session/load` on the **already-live** id still yields exactly one
    /// `factory.build` and one `SessionReplaced` (`ACP-212`) and still re-advertises commands,
    /// which is upstream's stated reason for closing the id first.
    ///
    /// # Errors
    ///
    /// [`AcpFailure::InvalidParams`] for a non-absolute cwd (`cwd must be an absolute path: <p>`,
    /// byte-for-byte) or an unknown id, and whatever the build failure classifies as.
    pub async fn prepare_load(&self, req: &LoadSessionRequest) -> Result<LoadOutcome, AcpFailure> {
        // `ACP-211` — first statement. `session/load` with a relative cwd performs no filesystem
        // work at all, which is what the unit's verify asserts.
        let cwd = AbsCwd::parse(req.cwd.clone())?;
        let stored = self.locate(&req.session_id)?;

        // Upstream's `opts?.cwd ?? stored.cwd`: the request's cwd wins, because a client that moved
        // a project is telling the agent where it is now.
        let runtime = self
            .gate
            .rebuild(|| {
                self.build_and_install(
                    &req.session_id,
                    SessionTarget::Resume(stored.file.path().to_path_buf()),
                    cwd.clone(),
                )
            })
            .await?;

        let session = runtime.session().await;
        let replay = replay_updates(&session.replay_items().await, &cwd);
        // `ACP-062` / `ACP-064` — the same one-read surface `session/new` advertises, so a client
        // that reloads a session is not left with a stale mode list from the session it left.
        let (modes, options) = crate::config_options::session_surface(&session).await;

        Ok(LoadOutcome {
            session_id: req.session_id.clone(),
            replay,
            response: LoadSessionResponse::new()
                .modes(modes)
                .config_options(options),
            // `ACP-217` — deliberately after the response, which is upstream's `setTimeout(fn, 0)`
            // and its stated reason: *"some clients (e.g. Zed) will ignore notifications for an
            // unknown sessionId"*. The command set itself is `crate::commands`' (`ACP-071`,
            // `ACP-267`…`ACP-272`); the ORDERING is this unit's, and it is what is asserted here.
            // `ACP-069` / `ACP-293` — `available_commands_update`, the ONE function both handlers
            // call. `merge_commands(Vec::new())` (this line before integration) advertised the
            // built-ins alone, so every prompt template, skill and extension command was missing
            // from the client's palette after a load and present after a new.
            follow_up: vec![crate::commands::available_commands_update(&session)],
        })
    }

    /// Drive `session/load` end to end: replay, then the response, then the command advertisement
    /// (`ACP-217`).
    ///
    /// This is the entry point `connection.rs`'s `session/load` arm must call. It is shaped like
    /// [`SessionManager::dispatch_prompt`] — responder in, `Result<(), Error>` out — because the
    /// ordering it enforces cannot be expressed by returning a value:
    /// [`crate::respond_then_notify`] writes the response first by construction, which is right for
    /// `session/new` and wrong for a load.
    ///
    /// # `ACP-Q35`, decided — replay from the spawned task, not from the dispatch loop
    ///
    /// The work runs inside `cx.spawn`, so a long replay cannot block an inbound `session/cancel`
    /// — which is the second half of `ACP-217`'s verify. Inside that task the notifications are
    /// `cx.send_notification`, which is **synchronous** (it enqueues on an mpsc and returns), so
    /// ordering against the response is guaranteed by program order rather than by a timer.
    ///
    /// The question asks whether a 10 k-entry session should instead stream, since this materialises
    /// every update before writing any. **It should not, here:** `AgentSession::replay_items()`
    /// already materialises the whole transcript under the manager lock, so the `Vec<SessionUpdate>`
    /// is bounded by a `Vec` that already exists, and streaming would mean holding that lock across
    /// the write. The cost is one transcript's worth of updates held twice, briefly.
    ///
    /// # Errors
    ///
    /// Never for a per-request failure — those are answered through `responder` and the spawned
    /// task still returns `Ok(())`, because `ConnectionTo::spawn`'s own doc says a task returning
    /// `Err` shuts the entire server down. Only a `spawn` refusal propagates.
    pub fn handle_load(
        self: &Arc<Self>,
        req: LoadSessionRequest,
        responder: Responder<LoadSessionResponse>,
        cx: ConnectionTo<Client>,
    ) -> Result<(), agent_client_protocol::Error> {
        let this = Arc::clone(self);
        let out = cx.clone();
        cx.spawn(async move {
            match this.prepare_load(&req).await {
                Ok(LoadOutcome {
                    session_id,
                    replay,
                    response,
                    follow_up,
                }) => {
                    for update in replay {
                        // `ACP-122` — a failed notification must not stop the load, mirroring
                        // upstream's unconditional silent `.catch(() => {})`.
                        let _ = out.send_notification(SessionNotification::new(
                            session_id.clone(),
                            update,
                        ));
                    }
                    let _ = responder.respond(response);
                    for update in follow_up {
                        let _ = out.send_notification(SessionNotification::new(
                            session_id.clone(),
                            update,
                        ));
                    }
                }
                Err(failure) => {
                    let _ = responder.respond_with_error(failure.into());
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    /// `session/load` in [`crate::HandlerOutcome`] shape, for the handler table as it stands today.
    ///
    /// # [CYRUP-DELTA] — this signature cannot express `ACP-217`, and that is why `handle_load` exists
    ///
    /// **What differs.** `ACP-217` requires every replay notification to be on the wire **before**
    /// the response, and `crate::respond_then_notify` — which is what `connection.rs` currently
    /// applies to this function's result — writes the response first, by construction and on
    /// purpose. `LoadSessionResponse` additionally names no session, so `SessionScoped` yields
    /// `None` and the follow-up is not addressable at all. Both facts are `lib.rs`'s and neither is
    /// a defect there.
    ///
    /// **What it costs.** Until `connection.rs`'s `session/load` arm is switched to
    /// [`SessionManager::handle_load`] — a four-line change filed as this module's interface change
    /// — a `session/load` restores the session correctly and emits **no** replay and **no**
    /// command advertisement. The transcript is not lost (it is on disk and in the rebuilt
    /// session); the client simply is not told it. This function returns both halves in
    /// `follow_up` so that a caller which *can* address them still gets them, in the only order
    /// this shape can carry.
    ///
    /// # Errors
    ///
    /// As [`SessionManager::prepare_load`].
    pub async fn load_session(
        &self,
        req: &LoadSessionRequest,
    ) -> Result<HandlerOutcome<LoadSessionResponse>, AcpFailure> {
        let outcome = self.prepare_load(req).await?;
        let mut follow_up = outcome.replay;
        follow_up.extend(outcome.follow_up);
        Ok(HandlerOutcome::with_follow_up(outcome.response, follow_up))
    }

    /// `session/list` (`ACP-203`…`ACP-208`, `ACP-223`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `listSessions`. One streaming pass over
    /// [`list_rows`] replaces `listPiSessions`'s five head/tail/whole-file scanning
    /// strategies, filtered by [`SessionManager::last_cwd`] when the request names no `cwd`
    /// (`ACP-207`) and paginated at 50 with a numeric-offset opaque cursor (`ACP-208`).
    ///
    /// The `cwd` default is upstream's own and its comment states the reason verbatim: *"Zed
    /// currently sends `{}` (no cwd), so we default to the last session cwd to emulate pi's
    /// `/resume` picker (project-scoped)."* With neither a request cwd nor a last cwd, every
    /// session across every project is returned.
    ///
    /// # `ACP-Q32`, decided — the offset cursor is kept
    ///
    /// An offset over a list re-scanned per request is not stable: a session touched between two
    /// pages re-sorts to the front, and the client skips one row and sees another twice. Upstream
    /// accepted that ("For MVP") and so does this, because the alternative — encoding
    /// `(updated_at, session_id)` — buys stability only against *concurrent writes to other
    /// sessions*, which for a one-live-session ACP host means the user typing in a different
    /// editor window. The field is opaque on the wire, so this is reversible without a protocol
    /// change, which is the reason it is safe to defer.
    ///
    /// # [CYRUP-DELTA] — `nextCursor` is omitted on the last page, not `null`
    ///
    /// **What differs.** Upstream returns `nextCursor: null` explicitly. `ListSessionsResponse` in
    /// schema 1.7.0 carries `#[skip_serializing_none]`, so `None` serializes as an **absent key**
    /// and there is no way to emit the literal `null` short of hand-writing the frame. `_meta` is
    /// still `{}` rather than absent, which `.meta(Meta::new())` is required for and which the ACP
    /// spec does distinguish.
    ///
    /// **What it costs.** A client testing `"nextCursor" in response` rather than
    /// `response.nextCursor == null` sees a difference. Both spellings mean "no more pages" to
    /// every client that reads the field's value, and `the_last_page_omits_the_cursor_and_keeps_meta`
    /// pins the actual bytes so the claim is checked rather than asserted.
    ///
    /// # Errors
    ///
    /// None. A listing that finds nothing is an empty response, not an error — including under a
    /// sessions root that does not exist, which is the first-run case.
    pub async fn list_sessions(
        &self,
        req: &ListSessionsRequest,
    ) -> Result<HandlerOutcome<ListSessionsResponse>, AcpFailure> {
        let root = self.host.sessions_root();
        let effective_cwd = match req.cwd.clone() {
            Some(cwd) => Some(cwd),
            None => self.last_cwd().await.map(|c| c.as_path().to_path_buf()),
        };

        // `ACP-207`'s note, made structural: the cwd filter and `ACP-206`'s projection drop both
        // run BEFORE the slice, because `nextCursor` is computed against the surviving length and
        // filtering after the slice silently shrinks pages.
        let rows: Vec<AcpSessionInfo> = list_rows(&root, effective_cwd.as_deref())
            .iter()
            .filter_map(to_acp_row)
            .collect();

        let start = parse_cursor(req.cursor.as_deref());
        let page: Vec<AcpSessionInfo> = rows.iter().skip(start).take(PAGE_SIZE).cloned().collect();
        let next = start.saturating_add(PAGE_SIZE);
        let next_cursor = (next < rows.len()).then(|| next.to_string());

        Ok(HandlerOutcome::plain(
            ListSessionsResponse::new(page)
                .next_cursor(next_cursor)
                // Required: without it the key is omitted, where upstream emits `"_meta":{}`.
                .meta(Meta::new()),
        ))
    }

    /// `session/delete` (`ACP-218`, `ACP-219`, `ACP-224`).
    ///
    /// Port of pi-acp v0.0.33 `agent.ts`'s `deleteSession`. Idempotent on an absent file — both of
    /// upstream's lookups missing means `{}`, success, no error and no write, citing the ACP
    /// semantics that deleting a non-existent session succeeds.
    ///
    /// # `ACP-219` / `ACP-Q37`, decided — the live session is disposed FIRST, and that is a
    /// deliberate divergence
    ///
    /// **What differs, and why it is not tidying.** Upstream does not distinguish: it unlinks
    /// whether or not a child holds the file, which is safe on POSIX because the child's fd keeps
    /// the inode alive. `ACP-224` records the consequence — after a successful delete pi-acp leaves
    /// the session live and a following `session/prompt` succeeds, appending to an unlinked inode.
    ///
    /// **cyrup's two mechanisms actively fight that shape.** `AgentSession::delete_session_file`
    /// returns `SessionServiceError::Io("refusing to delete the active session")`, an error ACP has
    /// no place for — so the free function `delete_session_file_at` is called instead, never the
    /// method. And `DiskStore` deliberately holds an `O_APPEND` fd precisely so a deleted file is
    /// not silently recreated as a headerless stub. Calling `delete_session_file_at` on the live
    /// session's path **without disposing first** leaves the session running, accepting prompts and
    /// appending every subsequent turn to an inode no listing, no `session/load` and no user can
    /// ever reach again — nothing errors, and the client sees a healthy session. That is silent
    /// data loss on a normal path (a session picker with a delete affordance, on the row that is
    /// open), which is why `ACP-219` is `critical` and why the sequence here is
    /// take-out-of-slot → `dispose` → unlink.
    ///
    /// **What it costs, exactly (`ACP-224`).** `dispose` emits `session_shutdown` and fires
    /// `session_cancel`, which kills tracked bash children. **A client that deletes a session it is
    /// mid-turn in loses the turn, where pi-acp would have completed it.** And a following
    /// `session/prompt` for that id gets `Unknown sessionId` rather than resurrecting the file.
    /// `ACP-219` and `ACP-224` state that pair as one rule so the two cannot disagree.
    ///
    /// # `ACP-Q36`, decided — `DeleteMethod` is not surfaced
    ///
    /// `delete_session_file_at` returns `Ok(DeleteMethod::Trash)` whenever the file merely no
    /// longer exists, so the value cannot distinguish "trashed" from "was already gone" and would
    /// be a misleading audit trail. The response stays `{}`, byte-for-byte upstream's.
    ///
    /// # [CYRUP-DELTA] — trash-first, against upstream's permanent unlink
    ///
    /// `delete_session_file_at` tries the `trash` CLI first (with pi's leading-dash `--` guard),
    /// treats exit-0 **or** the file having vanished as success, and falls back to `remove_file`.
    /// The cost is that a deleted ACP session is recoverable from the user's trash, which a client
    /// cannot observe and a user can. A failed unlink is swallowed and still answers `{}`, which is
    /// upstream's shape: the ACP contract permits no error here.
    ///
    /// # Errors
    ///
    /// [`AcpFailure::InvalidParams`] only for an id that is not a valid session id — a malformed
    /// request rather than an absent session, and the one case where the sanitiser's refusal should
    /// be visible rather than silently successful.
    pub async fn delete_session(
        &self,
        req: &DeleteSessionRequest,
    ) -> Result<HandlerOutcome<DeleteSessionResponse>, AcpFailure> {
        let id = AcpSessionId::parse(&req.session_id.0)?;
        let root = self.host.sessions_root();

        // `ACP-219` — if the target is the live session, take it out of the slot and dispose it
        // BEFORE anything touches the file. Its own `session_file()` is the authoritative path for
        // that case; `find_stored` is the fallback for a session that is merely on disk.
        let live_path = match self.take_live(&req.session_id).await {
            // `LiveSession::dispose_and_take_path` consumes the session: there is no other way to
            // learn the live session's path, so the dispose cannot be dropped or reordered here.
            Some(live) => live.dispose_and_take_path().await,
            None => None,
        };

        let file = live_path
            .and_then(|path| SessionFile::resolve(&root, &path).ok())
            .or_else(|| find_stored(&root, &id).map(|stored| stored.file));

        let Some(file) = file else {
            // Both lookups missed: idempotent success, no error, no write.
            return Ok(HandlerOutcome::plain(DeleteSessionResponse::new()));
        };

        if let Err(error) = delete_session_file_at(file.path()) {
            // Upstream's swallowing try/catch. The delete is best-effort and the response is
            // `{}` either way; the log is the only place the failure is visible at all.
            tracing::warn!(
                path = %file.path().display(),
                %error,
                "ACP-218: session/delete could not remove the session file"
            );
        }
        Ok(HandlerOutcome::plain(DeleteSessionResponse::new()))
    }
    /// `session/set_mode` (`ACP-072`, `ACP-078`, `ACP-079`).
    ///
    /// Sets the thinking level and echoes the **applied** one. A test asserting only that `{}`
    /// came back passes the broken version, which is why `ACP-072`'s verify is written against a
    /// model whose supported levels exclude the requested one — the clamp is
    /// [`crate::config_options::AppliedKnob`]'s job and the notification is the pump's
    /// (`ACP-Q20`).
    ///
    /// `follow_up` is deliberately empty: [`config_pump`] observes
    /// `AgentSessionEvent::ThinkingLevelChanged` and emits `current_mode_update` +
    /// `config_option_update` from there, so a level changed by an extension or by the TUI reaches
    /// the client through the same one emitter this does. Putting the notification here as well
    /// would double it for the ACP route only.
    ///
    /// # Errors
    ///
    /// [`SessionManager::unknown_session`] for a stale id, or `InvalidParams` for a mode this
    /// session does not offer.
    pub async fn set_mode(
        &self,
        req: &SetSessionModeRequest,
    ) -> Result<HandlerOutcome<SetSessionModeResponse>, AcpFailure> {
        let session = self.live_session(&req.session_id).await?;
        let (_applied, response) =
            crate::config_options::apply_mode(&session, &req.mode_id).await?;
        Ok(HandlerOutcome::plain(response))
    }

    /// `session/set_config_option` (`ACP-073`, `ACP-075`, `ACP-077`).
    ///
    /// Routes `model` and `thought_level` through
    /// [`crate::config_options::SessionConfigKnob`], and re-derives the WHOLE option set for the
    /// response so the advertised `currentValue` is the session's state read back independently,
    /// never the requested value (`ACP-075`).
    ///
    /// The push is [`config_pump`]'s, for the reason on [`SessionManager::set_mode`].
    ///
    /// # Errors
    ///
    /// `Unknown config option: <id>` at `-32602` for an unrecognised `configId`.
    pub async fn set_config_option(
        &self,
        req: &SetSessionConfigOptionRequest,
    ) -> Result<HandlerOutcome<SetSessionConfigOptionResponse>, AcpFailure> {
        let session = self.live_session(&req.session_id).await?;
        let (_applied, response) =
            crate::config_options::apply_config_option(&session, &req.config_id.0, &req.value)
                .await?;
        Ok(HandlerOutcome::plain(response))
    }

    /// The live [`AgentSession`] for `id`, restoring it from disk if this connection has moved on.
    ///
    /// `ACP-078`'s `Unknown sessionId` gate and `ACP-209`'s single-flight in one call. The setters
    /// use it rather than `get()` for a reason worth stating: a client that reconnects and sets a
    /// config option before prompting would otherwise be told its own session does not exist.
    ///
    /// # Errors
    ///
    /// As [`SessionManager::restore_session`].
    async fn live_session(&self, id: &SessionId) -> Result<Arc<AgentSession>, AcpFailure> {
        Ok(self.restore_session(id).await?.session().await)
    }

    /// `session/prompt` — hand the request and its responder to the turn (`ACP-121`, `ACP-153`).
    ///
    /// The whole body runs on a spawned task and this function returns immediately, which is
    /// `ACP-123`'s interleaving rule: a `session/cancel` issued straight after a `session/prompt`
    /// must be dispatched **before** the prompt response, and it is only if this handler does not
    /// block the loop.
    ///
    /// The responder is taken **by value** on purpose: `ACP-121`'s failure mode is a turn that is
    /// accepted and whose responder is then dropped, which leaves the editor's request permanently
    /// unanswered with no timeout on the ACP side — the user sees a spinner forever. Owning it
    /// makes "accepted a prompt and forgot the responder" a move error at the call site, and
    /// [`crate::turn::TurnHandle::prompt`] hands it **back** rather than dropping it when the
    /// actor is gone.
    ///
    /// # Errors
    ///
    /// Only a transport error. A refused prompt is answered through `responder`, and the spawned
    /// task returns `Ok(())` — the spawned-task contract (`ConnectionTo::spawn`'s own doc: *"if the
    /// spawned task returns an error, the entire server will shut down"*) means a per-request
    /// failure must never propagate.
    pub fn dispatch_prompt(
        self: &Arc<Self>,
        req: PromptRequest,
        responder: Responder<PromptResponse>,
        cx: ConnectionTo<Client>,
    ) -> Result<(), agent_client_protocol::Error> {
        let this = Arc::clone(self);
        cx.spawn(async move {
            this.serve_prompt(req, responder).await;
            Ok(())
        })?;
        Ok(())
    }

    /// `session/prompt`'s body, off the dispatch loop (`ACP-153`, `ACP-158`, `ACP-282`).
    ///
    /// Four steps, in the order upstream's `prompt()` has them:
    ///
    /// 1. **Translate the content blocks** ([`crate::commands::prompt_to_user_input`],
    ///    `translate/prompt.ts`). Before the session is resolved, because a malformed prompt should
    ///    not build a session.
    /// 2. **Resolve the session** — [`SessionManager::restore_session`], never a bare
    ///    `get()`-then-build: it is `ACP-209`'s single-flight, `ACP-225`'s live short-circuit and
    ///    `ACP-221`'s failure classification in one call.
    /// 3. **Intercept a built-in** ([`crate::commands::intercept`]). The dispatcher sits **above**
    ///    the turn queue, which is upstream's position for it, so `/compact` never enters the queue
    ///    and `ACP-292`'s refusal is decided by the command itself.
    /// 4. Otherwise hand the submission and the responder to the turn.
    ///
    /// **`ACP-266`, structural:** nothing here expands a prompt template. `/tpl $1` is not a
    /// built-in, so it is not intercepted, and it reaches `AgentSession::prepare_and_assemble` with
    /// its `$1` intact — which is where cyrup expands templates and why `slash-commands.ts` is cut
    /// outright.
    async fn serve_prompt(
        self: Arc<Self>,
        req: PromptRequest,
        responder: Responder<PromptResponse>,
    ) {
        // 1.
        let (text, images) = crate::commands::prompt_to_user_input(&req.prompt);

        // 2.
        let runtime = match self.restore_session(&req.session_id).await {
            Ok(runtime) => runtime,
            Err(failure) => {
                let _ = responder.respond_with_error(failure.into());
                return;
            }
        };

        // 3.
        if let Some((builtin, raw)) = crate::commands::intercept(&text, !images.is_empty()) {
            self.serve_builtin(&req.session_id, builtin, &raw, &runtime, responder)
                .await;
            return;
        }

        // 4.
        let Some(turn) = self.turn_for(&req.session_id) else {
            // The slot moved between the restore and this read — only possible if another handler
            // installed a different session in between, which for a one-live-session connection
            // means the client asked for one. The prompt is for a session that is no longer this
            // connection's, which is exactly `Unknown sessionId`.
            let _ = responder.respond_with_error(Self::unknown_session(&req.session_id).into());
            return;
        };
        let input = UserInput {
            text,
            images,
            // `ACP-025`'s sibling: an ACP client is an RPC-shaped, UI-bearing front-end, and
            // `InputSource` has no `Acp` variant. Adding one is a guest-visible change to every
            // consumer of `InputSource`; `Rpc` is what `ext_mode` already reports this front-end
            // as, so the two agree.
            source: InputSource::Rpc,
            // cyrup expands templates server-side (`ACP-266`). This is the flag that makes it so.
            expand_templates: true,
        };
        if let Err(responder) = turn.prompt(req.session_id.clone(), input, responder) {
            // The actor is gone (the session was replaced under this request). Answer rather than
            // drop: a dropped responder is a spinner forever.
            let _ = responder.respond_with_error(Self::unknown_session(&req.session_id).into());
        }
    }

    /// One built-in, dispatched above the turn queue (`ACP-282`, `ACP-292`).
    ///
    /// `ACP-264`, honoured: the raw argument remainder is handed to [`crate::commands::dispatch`],
    /// which splits it with `cyrup_resources::parse_command_args` — the workspace's one quote-aware
    /// tokenizer — rather than anything here re-implementing one.
    async fn serve_builtin(
        &self,
        session_id: &SessionId,
        builtin: crate::commands::Builtin,
        raw_args: &str,
        runtime: &Arc<AgentSessionRuntime>,
        responder: Responder<PromptResponse>,
    ) {
        // `ACP-291` — the id was already validated by `restore_session`'s `locate`, and it is
        // re-parsed rather than transmuted because `/export` composes a filesystem path from it and
        // an `AcpSessionId` is the only type allowed to do that.
        let parsed = match AcpSessionId::parse(&session_id.0) {
            Ok(parsed) => parsed,
            Err(failure) => {
                let _ = responder.respond_with_error(failure.into());
                return;
            }
        };
        let cwd = match self.live.lock().await.as_ref().map(|s| s.cwd.clone()) {
            Some(cwd) => cwd,
            None => {
                let _ = responder.respond_with_error(Self::unknown_session(session_id).into());
                return;
            }
        };
        let session = runtime.session().await;
        let rename_echo = self.rename_echo_for(session_id);
        match crate::commands::dispatch(builtin, raw_args, &parsed, &cwd, &session, &rename_echo)
            .await
        {
            Ok(updates) => {
                let wire = self.wire();
                for update in updates {
                    wire.notify(session_id, update);
                }
                // Every built-in ends the turn — upstream returns `stopReason: 'end_turn'` from
                // each arm, and none of them starts a run.
                let _ =
                    responder.respond(PromptResponse::new(crate::commands::BUILTIN_STOP_REASON));
            }
            Err(failure) => {
                let _ = responder.respond_with_error(failure.into());
            }
        }
    }

    /// `session/cancel` (`ACP-123`, `ACP-159`).
    ///
    /// Idempotent, and it never answers anything itself: the `stopReason: "cancelled"` is produced
    /// by the turn's own settle, from the state of **that** turn. Upstream reads a session-wide
    /// `cancelRequested` flag *after* awaiting the turn promise, i.e. after `startTurn` may already
    /// have cleared it for the queued successor — which is the bug `Turn`'s consuming settle makes
    /// unrepresentable.
    ///
    /// Takes `&self`, not `async`, because it is called from the notification handler, which runs
    /// **inside** the dispatch loop: anything awaited here delays every subsequent message. That is
    /// the whole reason [`SessionBinding`] lives behind a `std::sync::Mutex` rather than beside the
    /// session in the `tokio` one.
    ///
    /// A cancel for a session that is not live, or for no session at all, is a legal no-op —
    /// upstream included. There is nothing to answer.
    pub fn request_cancel(&self, id: &SessionId) {
        if let Some(turn) = self.turn_for(id) {
            turn.cancel();
        }
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
    use cyrup_core::{AssistantMessage, ProviderId, StopReason, ToolCallId as CoreToolCallId};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---------------------------------------------------------------------------------------
    // Fixtures
    // ---------------------------------------------------------------------------------------

    /// A host with a real sessions root and no ability to build. Every `session/*` path that stops
    /// at resolution is fully exercisable through it; the ones that need a live
    /// `AgentSessionRuntime` are named in this module's `tests_added` report as `cyrup-it` cases,
    /// because `AgentSessionRuntime` has no constructor short of a real provider-backed build.
    struct TestHost {
        root: PathBuf,
    }

    impl AcpHost for TestHost {
        fn build_runtime<'a>(
            &'a self,
            _req: &'a RuntimeRequest,
        ) -> BoxFuture<'a, Result<Arc<AgentSessionRuntime>, AcpError>> {
            Box::pin(async { Err(AcpError::Host("the test host builds nothing".into())) })
        }
        fn runtime_ready(&self, _runtime: &Arc<AgentSessionRuntime>) {}
        fn sessions_root(&self) -> SessionsRoot {
            SessionsRoot(self.root.clone())
        }
    }

    fn host_at(root: &Path) -> Arc<dyn AcpHost> {
        Arc::new(TestHost {
            root: root.to_path_buf(),
        })
    }

    /// Write a session JSONL: a header line, then whatever body lines the case needs.
    fn write_session(
        dir: &Path,
        file: &str,
        id: &str,
        cwd: &str,
        ts: &str,
        body: &[&str],
    ) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(file);
        let mut out = format!(
            r#"{{"type":"session","version":3,"id":"{id}","timestamp":"{ts}","cwd":"{cwd}"}}"#
        );
        for line in body {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        std::fs::write(&path, out).unwrap();
        path
    }

    fn user_line(text: &str, ts_ms: i64) -> String {
        format!(
            r#"{{"type":"message","id":"e1","parentId":null,"timestamp":"2024-01-02T03:04:06.000Z","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}],"timestamp":{ts_ms}}}}}"#
        )
    }

    fn info_line(name: Option<&str>) -> String {
        match name {
            Some(name) => format!(
                r#"{{"type":"session_info","id":"e2","parentId":"e1","timestamp":"2024-01-02T03:04:07.000Z","name":"{name}"}}"#
            ),
            None => r#"{"type":"session_info","id":"e2","parentId":"e1","timestamp":"2024-01-02T03:04:07.000Z","name":null}"#.to_string(),
        }
    }

    fn text(text: &str) -> Content {
        Content::Text {
            text: text.into(),
            text_signature: None,
        }
    }

    fn user(text_body: &str) -> ReplayItem {
        ReplayItem::Message(Box::new(AgentMessage::Core(Message::User {
            content: vec![text(text_body)],
            timestamp: 0,
        })))
    }

    fn assistant(text_body: &str) -> ReplayItem {
        let mut message = AssistantMessage::errored(
            ProviderId::from("test"),
            "test-model",
            None,
            StopReason::Stop,
            "",
        );
        message.content = vec![text(text_body)];
        ReplayItem::Message(Box::new(AgentMessage::Core(Message::Assistant(message))))
    }

    fn tool_result(
        id: &str,
        name: &str,
        body: &str,
        is_error: bool,
        details: Option<Value>,
    ) -> ReplayItem {
        ReplayItem::Message(Box::new(AgentMessage::Core(Message::ToolResult {
            tool_call_id: CoreToolCallId::from(id),
            tool_name: name.to_string(),
            content: if body.is_empty() {
                Vec::new()
            } else {
                vec![text(body)]
            },
            is_error,
            details,
            usage: None,
            added_tool_names: Vec::new(),
            timestamp: 0,
        })))
    }

    fn json_of(update: &SessionUpdate) -> Value {
        serde_json::to_value(update).unwrap()
    }

    /// `Result::unwrap_err` needs `T: Debug`, and neither `AgentSessionRuntime` nor
    /// `HandlerOutcome` is. This is the same assertion without the bound.
    fn err_of<T>(result: Result<T, AcpFailure>) -> AcpFailure {
        match result {
            Err(failure) => failure,
            Ok(_) => panic!("expected a failure, got a success"),
        }
    }

    // ---------------------------------------------------------------------------------------
    // ACP-120 / ACP-210 / the slot
    // ---------------------------------------------------------------------------------------

    /// ACP-120 / ACP-210 — the lookup-miss string is byte-for-byte upstream's, and is built by hand
    /// so `From<ErrorCode>` cannot stamp `"Invalid params"` over it.
    #[test]
    fn the_unknown_session_message_is_byte_exact() {
        let failure = SessionManager::unknown_session(&SessionId::new("bogus"));
        assert_eq!(
            failure,
            AcpFailure::InvalidParams {
                message: "Unknown sessionId: bogus".into()
            }
        );
        let wire: agent_client_protocol::Error = failure.into();
        assert_eq!(i32::from(wire.code), -32602);
        assert_eq!(wire.message, "Unknown sessionId: bogus");
    }

    /// The slot starts empty and a miss is a miss — no fabricated session, no panic.
    #[tokio::test]
    async fn a_fresh_manager_has_nothing_live() {
        let mgr = SessionManager::new(null_host());
        assert!(mgr.get(&SessionId::new("s1")).await.is_none());
        assert!(mgr.last_cwd().await.is_none());
    }

    /// ACP-207 — `lastSessionCwd` is connection-scoped state `session/new` writes and
    /// `session/list` reads.
    #[tokio::test]
    async fn last_cwd_round_trips() {
        let mgr = SessionManager::new(null_host());
        let cwd = AbsCwd::parse("/tmp/project").unwrap();
        mgr.set_last_cwd(cwd.clone()).await;
        assert_eq!(mgr.last_cwd().await, Some(cwd));
    }

    // ---------------------------------------------------------------------------------------
    // ACP-222 — the filename is a hint; the header decides
    // ---------------------------------------------------------------------------------------

    /// ACP-222, THE unit this whole area is gated on. Both of `uuid_of`'s failure shapes appear in
    /// pi-acp's own fixtures and both are reproduced here verbatim: `0000_delete_me.jsonl`
    /// (`test/unit/session-delete.test.ts`) derives `"me"`, and `s.jsonl`
    /// (`test/component/session-list-custom-session-dir.test.ts`) derives the whole stem `"s"`.
    ///
    /// The assertion is in both directions, because a resolver that simply never matched would pass
    /// half of it: the derived-but-wrong id must NOT resolve, and the real header id MUST.
    #[test]
    fn a_filename_derivation_never_decides_which_transcript_is_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        let dir = tmp.path().join("--proj-a--");
        write_session(
            &dir,
            "0000_delete_me.jsonl",
            "delete_me",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        write_session(
            &dir,
            "s.jsonl",
            "s-real",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );

        // `uuid_of("0000_delete_me")` is `"me"`; `uuid_of("s")` is `"s"`. Neither is a session id.
        assert!(find_stored(&root, &AcpSessionId::parse("me").unwrap()).is_none());
        assert!(find_stored(&root, &AcpSessionId::parse("s").unwrap()).is_none());

        // The header ids are, and they resolve to the right files.
        let a = find_stored(&root, &AcpSessionId::parse("delete_me").unwrap()).unwrap();
        assert!(a.file.path().ends_with("0000_delete_me.jsonl"));
        let b = find_stored(&root, &AcpSessionId::parse("s-real").unwrap()).unwrap();
        assert!(b.file.path().ends_with("s.jsonl"));
    }

    /// ACP-222's second half: `--session-id my_session` round-trips. `uuid_of` splits on the LAST
    /// underscore, so it derives `"session"` from this exact filename — the id a user chose must
    /// not become a different id, and the id it decays to must not resolve.
    #[test]
    fn a_user_chosen_session_id_containing_an_underscore_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        write_session(
            &tmp.path().join("--proj-a--"),
            "2024-01-02T03-04-05-000Z_my_session.jsonl",
            "my_session",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        assert!(find_stored(&root, &AcpSessionId::parse("session").unwrap()).is_none());
        let hit = find_stored(&root, &AcpSessionId::parse("my_session").unwrap()).unwrap();
        assert_eq!(hit.id.as_str(), "my_session");
        assert_eq!(
            hit.cwd.as_ref().map(|c| c.as_path().to_path_buf()),
            Some(PathBuf::from("/proj/a"))
        );
    }

    /// ACP-202 / ACP-201 — resolution crosses project directories and consults **only** the first
    /// parsed entry. The decoy is the assertion that no body is read: a line deep in the file that
    /// would itself parse as a session header must never become a match.
    #[test]
    fn resolution_crosses_projects_and_reads_no_session_bodies() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        for n in 0..5 {
            write_session(
                &tmp.path().join(format!("--proj-{n}--")),
                &format!("ts_p{n}.jsonl"),
                &format!("p{n}"),
                &format!("/proj/{n}"),
                "2024-01-02T03:04:05.000Z",
                &[
                    r#"{"type":"session","id":"decoy","timestamp":"2024-01-02T03:04:06.000Z","cwd":"/x"}"#,
                ],
            );
        }
        for n in 0..5 {
            let hit = find_stored(&root, &AcpSessionId::parse(&format!("p{n}")).unwrap()).unwrap();
            assert!(hit.file.path().ends_with(format!("ts_p{n}.jsonl")));
            assert_eq!(
                hit.cwd.as_ref().map(|c| c.as_path().to_path_buf()),
                Some(PathBuf::from(format!("/proj/{n}")))
            );
        }
        assert!(
            find_stored(&root, &AcpSessionId::parse("decoy").unwrap()).is_none(),
            "a header-shaped line in the BODY resolved — a session body was parsed"
        );
        assert!(find_stored(&root, &AcpSessionId::parse("nobody").unwrap()).is_none());
    }

    /// ACP-227 — a leading blank line. Upstream's `readFirstLine` + `if (!first) continue` drops
    /// the session entirely; cyrup's shared first-parsed-entry rule skips blanks, so it is listed
    /// AND resolvable. This is the recorded delta, and it is also what keeps `read_header_of`'s
    /// copy of that rule honest.
    #[test]
    fn a_leading_blank_line_does_not_hide_a_session() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        let dir = tmp.path().join("--proj-a--");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("ts_blank.jsonl"),
            "\n\n{\"type\":\"session\",\"version\":3,\"id\":\"blank\",\"timestamp\":\"2024-01-02T03:04:05.000Z\",\"cwd\":\"/proj/a\"}\n",
        )
        .unwrap();
        assert!(find_stored(&root, &AcpSessionId::parse("blank").unwrap()).is_some());
        assert_eq!(list_rows(&root, None).len(), 1);
    }

    /// A first parsed entry that is NOT a header stops the scan with no header — the other half of
    /// the rule, and the one that keeps a non-session `.jsonl` in the root out of every listing.
    #[test]
    fn a_file_whose_first_entry_is_not_a_header_is_not_a_session() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        std::fs::write(
            tmp.path().join("notes.jsonl"),
            "{\"type\":\"message\",\"id\":\"e1\",\"timestamp\":\"t\"}\n{\"type\":\"session\",\"id\":\"late\",\"timestamp\":\"t\",\"cwd\":\"/x\"}\n",
        )
        .unwrap();
        assert!(find_stored(&root, &AcpSessionId::parse("late").unwrap()).is_none());
        assert!(list_rows(&root, None).is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // ACP-223 / ACP-229 / ACP-230 — where the scan looks
    // ---------------------------------------------------------------------------------------

    /// ACP-223 — upstream's own fixture, ported. `settings.json {sessionDir}` with the session one
    /// cwd-encoded level BELOW the configured dir must be found; a flat-only scan (which is what
    /// `list_global_sessions` does under `session_dir_explicit`, and what `ACP-Q31` prescribed)
    /// returns zero here. Both levels are asserted in one root, because the real hazard is a user
    /// whose `sessionDir` points at the root: new sessions land flat and old ones are nested.
    #[test]
    fn the_scan_finds_both_the_flat_level_and_one_level_below_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        write_session(
            tmp.path(),
            "flat.jsonl",
            "flat",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        write_session(
            &tmp.path().join("--p--"),
            "s.jsonl",
            "nested",
            "/proj/a",
            "2024-01-02T03:04:06.000Z",
            &[],
        );

        let ids: Vec<String> = list_rows(&root, None)
            .iter()
            .map(|s| s.id.as_str().to_string())
            .collect();
        assert_eq!(
            ids,
            vec!["nested".to_string(), "flat".to_string()],
            "newest first"
        );
        assert!(find_stored(&root, &AcpSessionId::parse("flat").unwrap()).is_some());
        assert!(find_stored(&root, &AcpSessionId::parse("nested").unwrap()).is_some());
    }

    /// ACP-229 — a relative sessions root is refused rather than resolved against this process's
    /// working directory. The refusal is the whole decision: a relative root would otherwise make
    /// `session/delete` unlink files from wherever the editor happened to spawn cyrup.
    #[test]
    fn a_relative_sessions_root_scans_nothing_at_all() {
        assert!(session_dirs(&SessionsRoot(PathBuf::from("sessions-alt"))).is_empty());
        assert!(session_dirs(&SessionsRoot(PathBuf::new())).is_empty());
        assert!(list_rows(&SessionsRoot(PathBuf::from("sessions-alt")), None).is_empty());
        assert!(
            find_stored(
                &SessionsRoot(PathBuf::from("sessions-alt")),
                &AcpSessionId::parse("anything").unwrap()
            )
            .is_none()
        );
    }

    /// ACP-230 — a symlinked `*.jsonl` under the root. Upstream's `lstat`-based `walkJsonlFiles`
    /// skips it; cyrup lists it, and the decision is to keep cyrup's behaviour so the TUI and the
    /// ACP host do not disagree about which sessions exist.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_session_file_is_listed_and_resolvable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        let elsewhere = tmp.path().join("elsewhere");
        let real = write_session(
            &elsewhere,
            "real.jsonl",
            "linked",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        let dir = tmp.path().join("--proj-a--");
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&real, dir.join("link.jsonl")).unwrap();

        let hit = find_stored(&root, &AcpSessionId::parse("linked").unwrap()).unwrap();
        assert!(hit.file.path().ends_with("link.jsonl") || hit.file.path().ends_with("real.jsonl"));
        assert!(!list_rows(&root, None).is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // ACP-203…ACP-208 — the listing
    // ---------------------------------------------------------------------------------------

    /// **ACP-122 / ACP-285.** The rename update reaches the client exactly once, and the route
    /// that caused it can take responsibility for emitting it in order.
    ///
    /// Without this, the observed wire order for a `/name` turn was
    /// `agent_message_chunk` → `{"stopReason":"end_turn"}` → `session_info_update`, 9 runs of 9:
    /// the built-in is answered above the turn queue and the pump is a different task, so the
    /// notification always lost.
    #[test]
    fn a_claimed_rename_is_emitted_by_its_causer_and_not_by_the_pump() {
        let echo = crate::commands::RenameEcho::default();

        // Nobody claimed it — a rename from an extension or another front-end. The pump emits.
        let from_elsewhere = rename_update(Some("typed in the TUI".into()), &echo);
        assert_eq!(from_elsewhere.len(), 1);

        // `/name` claims before it mutates, then emits its own. The pump emits nothing.
        let claim = echo.claim();
        claim.keep();
        assert!(
            rename_update(Some("typed over ACP".into()), &echo).is_empty(),
            "two producers means the client renders the rename twice"
        );

        // …and exactly one claim is consumed, so the next rename is the pump's again.
        assert_eq!(rename_update(None, &echo).len(), 1);
    }

    /// The claim is released when the mutation fails, so a rejected `/name` cannot swallow
    /// somebody else's rename.
    #[test]
    fn an_unkept_rename_claim_releases_itself() {
        let echo = crate::commands::RenameEcho::default();
        drop(echo.claim());
        assert_eq!(
            rename_update(Some("still mine".into()), &echo).len(),
            1,
            "a dropped claim must not silence the next rename"
        );

        // Two overlapping `/name`s: a counter, not a slot, so neither is lost.
        let first = echo.claim();
        let second = echo.claim();
        first.keep();
        second.keep();
        assert!(rename_update(None, &echo).is_empty());
        assert!(rename_update(None, &echo).is_empty());
        assert_eq!(rename_update(None, &echo).len(), 1);
    }

    /// One `updatedAt` format on the wire, not two.
    ///
    /// `session/list` rows and `session_info_update` describe the same field of the same row, and
    /// the notification was emitting nine fractional digits where the rows emit three. Both
    /// producers are checked against `ACP-204`'s regex here so they cannot drift again.
    #[test]
    fn the_two_updated_at_producers_agree_on_precision() {
        let from_notification = now_iso8601_millis();
        let from_a_row = iso8601_millis(SystemTime::now()).expect("after the epoch");
        for stamp in [&from_notification, &from_a_row] {
            // `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$`, spelled out so no regex dependency
            // is needed — the same shape `updated_at_is_a_js_to_iso_string` pins.
            assert_eq!(stamp.len(), 24, "{stamp}");
            assert!(stamp.ends_with('Z'), "{stamp}");
            assert_eq!(&stamp[10..11], "T", "{stamp}");
            assert_eq!(&stamp[19..20], ".", "{stamp}");
            assert!(
                stamp[20..23].chars().all(|c| c.is_ascii_digit()),
                "exactly three fractional digits: {stamp}"
            );
        }

        // And the value the notification actually carries is that string, not a nanosecond one.
        let update =
            crate::config_options::session_info_update(Some("n".into()), now_iso8601_millis());
        let json = serde_json::to_value(&update).expect("serialises");
        let wire = json["updatedAt"].as_str().expect("a string");
        assert_eq!(wire.len(), 24, "{wire}");
    }

    /// ACP-204 — every row's `updatedAt` is JS `toISOString()`-shaped. The regex in the unit's
    /// verify is reproduced by hand so this test needs no dependency, and the epoch case pins the
    /// civil-from-days conversion against a value anyone can check.
    #[test]
    fn updated_at_is_a_js_to_iso_string() {
        assert_eq!(
            iso8601_millis(SystemTime::UNIX_EPOCH).unwrap(),
            "1970-01-01T00:00:00.000Z"
        );
        assert_eq!(
            iso8601_millis(
                SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_704_164_645_678)
            )
            .unwrap(),
            "2024-01-02T03:04:05.678Z"
        );
        // A leap day, because February is where a hand-rolled calendar breaks.
        assert_eq!(
            iso8601_millis(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800))
                .unwrap(),
            "2024-02-29T00:00:00.000Z"
        );

        let rendered = iso8601_millis(SystemTime::now()).unwrap();
        let bytes = rendered.as_bytes();
        assert_eq!(rendered.len(), 24, "{rendered}");
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert_eq!(bytes[10], b'T');
        assert_eq!(bytes[13], b':');
        assert_eq!(bytes[16], b':');
        assert_eq!(bytes[19], b'.');
        assert_eq!(bytes[23], b'Z');
        assert!(
            rendered
                .chars()
                .enumerate()
                .all(|(i, c)| matches!(i, 4 | 7 | 10 | 13 | 16 | 19 | 23) || c.is_ascii_digit()),
            "{rendered}"
        );
    }

    /// ACP-203 — `updatedAt` is the last message activity, not the header time. Ported in spirit
    /// from `test/component/session-updatedAt-message-only.test.ts`: a session whose header is old
    /// and whose only message is recent must sort and report by the message. The ladder itself is
    /// `listing::scan_file`'s (activity → header → mtime) and is not re-ported; this pins that the
    /// ACP projection reads the right field of it.
    #[test]
    fn updated_at_follows_the_last_message_not_the_header() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        let dir = tmp.path().join("--proj-a--");
        // Header at 2024-01-02T03:04:05Z; the one message is a day later.
        write_session(
            &dir,
            "a.jsonl",
            "recent",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[&user_line("hello", 1_704_251_045_678)],
        );
        // A header-only session written LATER by header time, to prove the ordering key changed.
        write_session(
            &dir,
            "b.jsonl",
            "older",
            "/proj/a",
            "2024-01-02T04:00:00.000Z",
            &[],
        );

        let rows = list_rows(&root, None);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["recent", "older"],
            "message activity outranks the header time"
        );
        let projected = to_acp_row(&rows[0]).unwrap();
        assert_eq!(
            projected.updated_at.as_deref(),
            Some("2024-01-03T03:04:05.678Z")
        );
    }

    /// ACP-205 — the title ladder, all four rungs. The header-only case is `null` and NOT the
    /// `(no messages)` sentinel `listing::scan_file` substitutes; the 200-character first message is
    /// clipped to exactly 80.
    #[test]
    fn the_title_ladder_clips_to_eighty_and_never_leaks_the_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        let dir = tmp.path().join("--proj-a--");
        let long = "x".repeat(200);

        write_session(
            &dir,
            "a.jsonl",
            "header-only",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        write_session(
            &dir,
            "b.jsonl",
            "long-first",
            "/proj/a",
            "2024-01-02T03:04:06.000Z",
            &[&user_line(&long, 1_704_164_646_000)],
        );
        write_session(
            &dir,
            "c.jsonl",
            "named",
            "/proj/a",
            "2024-01-02T03:04:07.000Z",
            &[
                &user_line("hello", 1_704_164_647_000),
                &info_line(Some("My session")),
            ],
        );

        let by_id: std::collections::HashMap<String, Option<String>> = list_rows(&root, None)
            .iter()
            .filter_map(to_acp_row)
            .map(|row| (row.session_id.to_string(), row.title.clone()))
            .collect();

        assert_eq!(
            by_id["header-only"], None,
            "the sentinel must not reach the wire"
        );
        assert_eq!(by_id["long-first"], Some("x".repeat(80)));
        assert_eq!(by_id["named"], Some("My session".to_string()));
    }

    /// ACP-228 — a session named and later cleared reports `null`, not the old name. Recorded as a
    /// delta: upstream's two title scanners both take the newest non-blank `session_info.name` and
    /// therefore keep showing the cleared name, because a `null` is not "newest non-blank".
    #[test]
    fn clearing_a_session_name_clears_the_title() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        write_session(
            &tmp.path().join("--proj-a--"),
            "a.jsonl",
            "cleared",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[&info_line(Some("Old name")), &info_line(None)],
        );
        let rows = list_rows(&root, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(title_of(&rows[0]), None);
    }

    /// ACP-206 — `SessionInfo.cwd` is a required absolute path. A header with no cwd stays in
    /// `listing` (cyrup's read-tolerance is unchanged) and is absent from `session/list`; there is
    /// no `"cwd": ""` row on the wire.
    #[test]
    fn a_session_with_no_absolute_cwd_is_listed_by_cyrup_and_not_by_acp() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().to_path_buf());
        let dir = tmp.path().join("--proj-a--");
        write_session(
            &dir,
            "a.jsonl",
            "nocwd",
            "",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        write_session(
            &dir,
            "b.jsonl",
            "relcwd",
            "relative/dir",
            "2024-01-02T03:04:06.000Z",
            &[],
        );
        write_session(
            &dir,
            "c.jsonl",
            "good",
            "/proj/a",
            "2024-01-02T03:04:07.000Z",
            &[],
        );

        let rows = list_rows(&root, None);
        assert_eq!(rows.len(), 3, "cyrup's own listing keeps all three");
        let acp: Vec<AcpSessionInfo> = rows.iter().filter_map(to_acp_row).collect();
        assert_eq!(acp.len(), 1);
        assert_eq!(acp[0].session_id.to_string(), "good");
        let wire = serde_json::to_string(&acp[0]).unwrap();
        assert!(!wire.contains(r#""cwd":"""#), "{wire}");
    }

    /// ACP-207 — the default cwd filter, ported from `test/component/session-list-scoped.test.ts`:
    /// `{}` with a last cwd returns only that cwd's session, an explicit `cwd` overrides it, and
    /// `{}` with no last cwd returns both.
    #[tokio::test]
    async fn the_default_cwd_filter_is_the_last_session_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(
            &tmp.path().join("--proj-a--"),
            "a.jsonl",
            "in-a",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        write_session(
            &tmp.path().join("--proj-b--"),
            "b.jsonl",
            "in-b",
            "/proj/b",
            "2024-01-02T03:04:06.000Z",
            &[],
        );
        let mgr = SessionManager::new(host_at(tmp.path()));

        let ids = |out: HandlerOutcome<ListSessionsResponse>| {
            out.response
                .sessions
                .iter()
                .map(|s| s.session_id.to_string())
                .collect::<Vec<_>>()
        };

        // No last cwd: every session, across every project.
        let all = mgr
            .list_sessions(&ListSessionsRequest::new())
            .await
            .unwrap();
        assert_eq!(ids(all), vec!["in-b".to_string(), "in-a".to_string()]);

        // A last cwd scopes `{}` to that project — upstream's `/resume`-picker emulation.
        mgr.set_last_cwd(AbsCwd::parse("/proj/a").unwrap()).await;
        let scoped = mgr
            .list_sessions(&ListSessionsRequest::new())
            .await
            .unwrap();
        assert_eq!(ids(scoped), vec!["in-a".to_string()]);

        // An explicit cwd overrides the default.
        let explicit = mgr
            .list_sessions(&ListSessionsRequest::new().cwd(PathBuf::from("/proj/b")))
            .await
            .unwrap();
        assert_eq!(ids(explicit), vec!["in-b".to_string()]);
    }

    /// ACP-208's cursor arithmetic, on the three inputs the unit names plus `parseInt`'s leniency.
    #[test]
    fn every_malformed_cursor_means_page_one() {
        assert_eq!(parse_cursor(None), 0);
        assert_eq!(parse_cursor(Some("abc")), 0, "NaN");
        assert_eq!(parse_cursor(Some("-5")), 0, "negative");
        assert_eq!(parse_cursor(Some("0")), 0, "zero");
        assert_eq!(parse_cursor(Some("")), 0);
        assert_eq!(parse_cursor(Some("50")), 50);
        assert_eq!(parse_cursor(Some("  50")), 50, "parseInt trims");
        assert_eq!(
            parse_cursor(Some("50abc")),
            50,
            "parseInt stops at the first non-digit"
        );
        assert_eq!(parse_cursor(Some("0x10")), 0, "parseInt with radix 10");
        assert_eq!(parse_cursor(Some("99999999999999999999999")), usize::MAX);
    }

    /// ACP-208 — 120 sessions, three pages. Page 1 has 50 rows and `nextCursor == "50"`, page 3 has
    /// 20 and none, and the last page's JSON keeps `"_meta":{}`.
    #[tokio::test]
    async fn pagination_is_fifty_rows_with_a_numeric_offset_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("--proj-a--");
        for n in 0..120u32 {
            write_session(
                &dir,
                &format!("ts_s{n:03}.jsonl"),
                &format!("s{n:03}"),
                "/proj/a",
                &format!("2024-01-02T03:04:{:02}.000Z", n % 60),
                &[],
            );
        }
        let mgr = SessionManager::new(host_at(tmp.path()));

        let page1 = mgr
            .list_sessions(&ListSessionsRequest::new())
            .await
            .unwrap()
            .response;
        assert_eq!(page1.sessions.len(), 50);
        assert_eq!(page1.next_cursor.as_deref(), Some("50"));

        let page2 = mgr
            .list_sessions(&ListSessionsRequest::new().cursor("50".to_string()))
            .await
            .unwrap()
            .response;
        assert_eq!(page2.sessions.len(), 50);
        assert_eq!(page2.next_cursor.as_deref(), Some("100"));

        let page3 = mgr
            .list_sessions(&ListSessionsRequest::new().cursor("100".to_string()))
            .await
            .unwrap()
            .response;
        assert_eq!(page3.sessions.len(), 20);
        assert_eq!(page3.next_cursor, None);

        for bogus in ["abc", "-5", "0"] {
            let first = mgr
                .list_sessions(&ListSessionsRequest::new().cursor(bogus.to_string()))
                .await
                .unwrap()
                .response;
            assert_eq!(first.sessions.len(), 50, "{bogus}");
            assert_eq!(first.next_cursor.as_deref(), Some("50"), "{bogus}");
        }
    }

    /// ACP-208's wire shape, measured rather than assumed. `_meta` MUST be `{}` and not absent;
    /// `nextCursor` is **omitted** on the last page rather than `null`, because
    /// `ListSessionsResponse` carries `#[skip_serializing_none]` in schema 1.7.0 — the recorded
    /// delta, pinned here so a schema bump that changes it is a failing test rather than a surprise.
    #[tokio::test]
    async fn the_last_page_omits_the_cursor_and_keeps_meta() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(host_at(tmp.path()));
        let response = mgr
            .list_sessions(&ListSessionsRequest::new())
            .await
            .unwrap()
            .response;
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(r#""_meta":{}"#), "{json}");
        assert!(!json.contains("nextCursor"), "{json}");
        assert!(json.contains(r#""sessions":[]"#), "{json}");
    }

    /// An absent sessions root is an empty listing, not an error — the first-run case.
    #[tokio::test]
    async fn a_sessions_root_that_does_not_exist_lists_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(host_at(&tmp.path().join("never-created")));
        let response = mgr
            .list_sessions(&ListSessionsRequest::new())
            .await
            .unwrap()
            .response;
        assert!(response.sessions.is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // ACP-209 / ACP-225 — single-flight restore, and the two entry points
    // ---------------------------------------------------------------------------------------

    /// ACP-209, THE critical one. Two concurrent restores of the same not-currently-live id build
    /// **exactly once** and both callers are answered with the same session.
    ///
    /// The slot and the counter stand in for `AgentSessionRuntime`, which has no constructor short
    /// of a real provider-backed build; what is under test is the gate, and the gate is what the
    /// unit is about. The `sleep` inside `build` is the await window that upstream's
    /// single-threaded tick closed for free and that Rust does not.
    #[tokio::test]
    async fn two_concurrent_restores_build_exactly_once() {
        let gate = RestoreGate::default();
        let slot: tokio::sync::Mutex<Option<u32>> = tokio::sync::Mutex::new(None);
        let builds = AtomicUsize::new(0);

        let build = || async {
            builds.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let mut slot = slot.lock().await;
            *slot = Some(7);
            Ok::<u32, AcpFailure>(7)
        };
        let live = || async { *slot.lock().await };

        let (a, b) = tokio::join!(gate.enter(live, build), gate.enter(live, build));
        assert_eq!(a.unwrap(), 7);
        assert_eq!(b.unwrap(), 7);
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "two prompts for one unloaded session built it twice"
        );
    }

    /// ACP-225 — the rule, asserted both ways in one test so the two units cannot be implemented
    /// independently into disagreement: `session/prompt` short-circuits on live, `session/load`
    /// bypasses the short-circuit.
    #[tokio::test]
    async fn prompt_short_circuits_on_live_and_load_does_not() {
        let gate = RestoreGate::default();
        let builds = AtomicUsize::new(0);
        let build = || async {
            builds.fetch_add(1, Ordering::SeqCst);
            Ok::<u32, AcpFailure>(1)
        };

        // Live: the prompt path returns it without building.
        let live_some = || async { Some(99u32) };
        assert_eq!(gate.enter(live_some, build).await.unwrap(), 99);
        assert_eq!(builds.load(Ordering::SeqCst), 0);

        // The same live session, through the load path: a build happens anyway (`ACP-212`).
        assert_eq!(gate.rebuild(build).await.unwrap(), 1);
        assert_eq!(builds.load(Ordering::SeqCst), 1);

        // Not live: the prompt path builds.
        let live_none = || async { None::<u32> };
        assert_eq!(gate.enter(live_none, build).await.unwrap(), 1);
        assert_eq!(builds.load(Ordering::SeqCst), 2);
    }

    /// A failed build is not cached: the next caller retries, which is upstream's
    /// `finally { restoringSessions.delete(id) }` without the map.
    #[tokio::test]
    async fn a_failed_restore_is_retried_rather_than_remembered() {
        let gate = RestoreGate::default();
        let builds = AtomicUsize::new(0);
        let build = || async {
            builds.fetch_add(1, Ordering::SeqCst);
            Err::<u32, AcpFailure>(AcpFailure::Internal {
                message: "nope".into(),
            })
        };
        let live = || async { None::<u32> };
        assert!(gate.enter(live, build).await.is_err());
        assert!(gate.enter(live, build).await.is_err());
        assert_eq!(builds.load(Ordering::SeqCst), 2);
    }

    /// ACP-210 / ACP-221 — restore's two failure shapes. An unknown id is the byte-exact message; a
    /// resolvable id whose build fails is an error the connection survives, classified rather than
    /// panicked, and NOT an auth prompt.
    #[tokio::test]
    async fn restore_maps_an_unknown_id_and_a_failed_build_to_distinct_errors() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(
            &tmp.path().join("--proj-a--"),
            "a.jsonl",
            "real",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        let mgr = SessionManager::new(host_at(tmp.path()));

        assert_eq!(
            err_of(mgr.restore_session(&SessionId::new("ghost")).await),
            AcpFailure::InvalidParams {
                message: "Unknown sessionId: ghost".into()
            }
        );
        assert!(matches!(
            mgr.restore_session(&SessionId::new("real")).await,
            Err(AcpFailure::Internal { .. })
        ));
        // ACP-291 — a malformed id is a malformed request, not a missing session.
        let malformed = err_of(mgr.restore_session(&SessionId::new("../etc/passwd")).await);
        assert!(
            matches!(&malformed, AcpFailure::InvalidParams { message } if message.starts_with("Session id must be")),
            "{malformed:?}"
        );
    }

    /// A session whose header records no absolute cwd cannot be restored — and says so, rather than
    /// resolving every later path against this process's working directory (`AbsCwd`'s third entry
    /// point, which upstream leaves unguarded).
    #[tokio::test]
    async fn a_session_with_no_recorded_cwd_is_not_restored_against_the_process_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(
            &tmp.path().join("--x--"),
            "a.jsonl",
            "nocwd",
            "",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        let mgr = SessionManager::new(host_at(tmp.path()));
        let err = err_of(mgr.restore_session(&SessionId::new("nocwd")).await);
        assert!(
            matches!(&err, AcpFailure::Internal { message } if message.contains("no absolute working directory")),
            "{err:?}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // ACP-211 / ACP-226 — session/load's guards and statement order
    // ---------------------------------------------------------------------------------------

    /// ACP-211 — `session/load` with a relative cwd returns the exact message, and it is the FIRST
    /// statement, so a request that also names a nonexistent session still fails on the cwd.
    #[tokio::test]
    async fn load_rejects_a_relative_cwd_with_upstreams_message() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(host_at(tmp.path()));
        let err = err_of(
            mgr.prepare_load(&LoadSessionRequest::new(
                SessionId::new("ghost"),
                "relative/dir",
            ))
            .await,
        );
        assert_eq!(
            err,
            AcpFailure::InvalidParams {
                message: "cwd must be an absolute path: relative/dir".into()
            }
        );
    }

    /// ACP-226 — a `session/load` for an unresolvable id must not re-scope the default
    /// `session/list` filter. Upstream writes `lastSessionCwd` before `findStoredSession` and
    /// therefore does.
    #[tokio::test]
    async fn an_unresolvable_load_does_not_rescope_the_listing_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(host_at(tmp.path()));
        mgr.set_last_cwd(AbsCwd::parse("/proj/a").unwrap()).await;

        let err = err_of(
            mgr.prepare_load(&LoadSessionRequest::new(SessionId::new("ghost"), "/proj/b"))
                .await,
        );
        assert_eq!(
            err,
            AcpFailure::InvalidParams {
                message: "Unknown sessionId: ghost".into()
            }
        );
        assert_eq!(
            mgr.last_cwd().await,
            Some(AbsCwd::parse("/proj/a").unwrap()),
            "ACP-226: the failed load re-scoped the default session/list filter"
        );
    }

    // ---------------------------------------------------------------------------------------
    // ACP-218 / ACP-219 / ACP-224 — session/delete
    // ---------------------------------------------------------------------------------------

    /// ACP-218 — the four cases from `test/unit/session-delete.test.ts`: an existing file is
    /// removed and answers `{}`; a second delete of the same id answers `{}`; an id nothing carries
    /// answers `{}`; and the response is `{}` in every case, never an error.
    #[tokio::test]
    async fn delete_removes_the_file_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_session(
            &tmp.path().join("--proj-a--"),
            "0000_delete_me.jsonl",
            "delete_me",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        let mgr = SessionManager::new(host_at(tmp.path()));

        mgr.delete_session(&DeleteSessionRequest::new(SessionId::new("delete_me")))
            .await
            .unwrap();
        assert!(!path.exists(), "the session file survived session/delete");

        // Idempotent: the same id again, and an id that never existed.
        for id in ["delete_me", "never-existed"] {
            let again = mgr
                .delete_session(&DeleteSessionRequest::new(SessionId::new(id)))
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_string(&again.response).unwrap(),
                "{}",
                "ACP-218: deleting an absent session must succeed with `{{}}`"
            );
            assert!(again.follow_up.is_empty());
        }
    }

    /// ACP-291 at the delete boundary — a malformed id never becomes a path, and the refusal is
    /// visible rather than a silent success.
    #[tokio::test]
    async fn delete_refuses_a_malformed_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(host_at(tmp.path()));
        for hostile in ["../../etc/passwd", "/etc/passwd", "", "a/b", ".."] {
            let err = err_of(
                mgr.delete_session(&DeleteSessionRequest::new(SessionId::new(hostile)))
                    .await,
            );
            assert!(
                matches!(&err, AcpFailure::InvalidParams { message } if message.starts_with("Session id must be")),
                "{hostile:?} -> {err:?}"
            );
        }
    }

    // ---------------------------------------------------------------------------------------
    // ACP-220 — the partial-session-file purge
    // ---------------------------------------------------------------------------------------

    /// ACP-220 — the mechanism `cleanup_failed_new_session` applies after disposing: the file is
    /// gone, an absent file is success, and a path outside the sessions root is REFUSED rather than
    /// unlinked. The last clause is the one that matters: this is a delete primitive.
    #[test]
    fn a_partial_session_file_is_purged_only_from_inside_the_sessions_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SessionsRoot(tmp.path().join("sessions"));
        let inside = write_session(
            &root.path().join("--proj-a--"),
            "stub.jsonl",
            "stub",
            "/proj/a",
            "2024-01-02T03:04:05.000Z",
            &[],
        );
        assert!(purge_partial_session_file(&root, &inside));
        assert!(!inside.exists());

        // Already gone is success — the file may never have materialised.
        assert!(purge_partial_session_file(&root, &inside));

        let outside = tmp.path().join("precious.jsonl");
        std::fs::write(&outside, "keep me").unwrap();
        assert!(!purge_partial_session_file(&root, &outside));
        assert!(
            outside.exists(),
            "ACP-220 unlinked a file outside the sessions root"
        );

        // Not a `.jsonl`, inside the root: still refused, because the extension check is what stops
        // a containment-passing `settings.json` from reaching a delete.
        let settings = root.path().join("settings.json");
        std::fs::write(&settings, "{}").unwrap();
        assert!(!purge_partial_session_file(&root, &settings));
        assert!(settings.exists());
    }

    // ---------------------------------------------------------------------------------------
    // ACP-214 / ACP-215 / ACP-216 — replay
    // ---------------------------------------------------------------------------------------

    /// ACP-214 — user and assistant text replay in transcript order, and a message with no text
    /// blocks emits nothing at all.
    #[test]
    fn replay_emits_user_and_assistant_text_in_order_and_nothing_for_an_empty_message() {
        let cwd = AbsCwd::parse("/proj/a").unwrap();
        let empty = ReplayItem::Message(Box::new(AgentMessage::Core(Message::User {
            content: vec![Content::Image {
                data: "AA==".into(),
                mime_type: "image/png".into(),
            }],
            timestamp: 0,
        })));
        let out = replay_updates(
            &[user("first"), assistant("second"), empty, user("third")],
            &cwd,
        );

        let json: Vec<Value> = out.iter().map(json_of).collect();
        assert_eq!(json.len(), 3, "{json:?}");
        assert_eq!(json[0]["sessionUpdate"], "user_message_chunk");
        assert_eq!(json[0]["content"]["text"], "first");
        assert_eq!(json[1]["sessionUpdate"], "agent_message_chunk");
        assert_eq!(json[1]["content"]["text"], "second");
        assert_eq!(json[2]["content"]["text"], "third");
    }

    /// The four roles cyrup has and ACP has no shape for are skipped deliberately, so the replay
    /// stream matches upstream's `role === 'user' | 'assistant' | 'toolResult'` branch set.
    #[test]
    fn the_roles_acp_cannot_render_are_skipped_rather_than_invented() {
        let cwd = AbsCwd::parse("/proj/a").unwrap();
        let bash: AgentMessage = serde_json::from_value(serde_json::json!({
            "role": "bashExecution",
            "command": "ls",
            "output": "a\nb\n",
            "timestamp": 0
        }))
        .unwrap();
        let out = replay_updates(
            &[ReplayItem::Message(Box::new(bash)), user("only me")],
            &cwd,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(json_of(&out[0])["content"]["text"], "only me");
    }

    /// ACP-215, ported from `test/component/session-load-toolresult.test.ts`: four notifications in
    /// order, `failed` on the errored one, and **both** updates in a pair carrying the same
    /// `toolCallId` — the persisted one, so it is stable across two loads and equal to the live one.
    #[test]
    fn a_replayed_tool_result_is_a_pair_sharing_the_persisted_id() {
        let cwd = AbsCwd::parse("/proj/a").unwrap();
        let items = vec![
            tool_result("call-ok", "read", "file contents", false, None),
            tool_result("call-bad", "write", "boom", true, None),
        ];
        let first = replay_updates(&items, &cwd);
        let second = replay_updates(&items, &cwd);

        let json: Vec<Value> = first.iter().map(json_of).collect();
        assert_eq!(json.len(), 4, "{json:?}");

        assert_eq!(json[0]["sessionUpdate"], "tool_call");
        assert_eq!(json[0]["toolCallId"], "call-ok");
        assert_eq!(json[0]["title"], "read");
        assert_eq!(json[0]["kind"], "read");
        assert_eq!(json[1]["sessionUpdate"], "tool_call_update");
        assert_eq!(json[1]["toolCallId"], "call-ok");
        assert_eq!(json[1]["status"], "completed");
        assert_eq!(json[1]["content"][0]["content"]["text"], "file contents");

        assert_eq!(json[2]["toolCallId"], "call-bad");
        assert_eq!(json[2]["kind"], "edit");
        assert_eq!(json[3]["toolCallId"], "call-bad");
        assert_eq!(
            json[3]["status"], "failed",
            "the errored result must downgrade"
        );

        // Stable across two loads: no `crypto.randomUUID()` fallback exists here.
        let second_json: Vec<Value> = second.iter().map(json_of).collect();
        assert_eq!(json, second_json);
    }

    /// ACP-Q34, decided — the replay `kind` mapping is `ToolClass::of`, the SAME classifier the
    /// live path uses (`ACP-151`), not upstream's three-name ladder. A replayed `grep` renders as
    /// `search`, where pi-acp rendered `other`; the point of the test is that replay and live agree.
    #[test]
    fn replayed_tool_kinds_are_the_live_classifiers_kinds() {
        let cwd = AbsCwd::parse("/proj/a").unwrap();
        for (name, kind) in [
            ("read", "read"),
            ("edit", "edit"),
            ("write", "edit"),
            ("grep", "search"),
            ("ls", "search"),
            ("bash", "execute"),
            ("some_mcp_tool", "other"),
        ] {
            let out = replay_updates(&[tool_result("c1", name, "x", false, None)], &cwd);
            // `ToolCall.kind` carries `skip_serializing_if = "ToolKind::is_default"` in schema
            // 1.7.0, so `other` is an ABSENT key on the wire rather than `"kind":"other"`. Pinned
            // here because a reader comparing raw frames would otherwise call it a missing field.
            let on_the_wire = json_of(&out[0])
                .get("kind")
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "other".to_string());
            assert_eq!(on_the_wire, kind, "{name}");
            assert_eq!(
                serde_json::to_value(ToolClass::of(name).acp_kind()).unwrap(),
                Value::from(kind),
                "replay and live disagree about {name}"
            );
        }
    }

    /// ACP-136's ladder reaches replay too: a persisted `edit` whose `details.diff` carries the
    /// unified diff renders the diff, exactly as the live path does.
    #[test]
    fn a_replayed_edit_shows_its_persisted_diff() {
        let cwd = AbsCwd::parse("/proj/a").unwrap();
        let details = serde_json::json!({ "diff": "--- a\n+++ b\n" });
        let out = replay_updates(
            &[tool_result(
                "c1",
                "edit",
                "Edited 1 file",
                false,
                Some(details),
            )],
            &cwd,
        );
        assert_eq!(
            json_of(&out[1])["content"][0]["content"]["text"],
            "--- a\n+++ b\n"
        );
    }

    /// ACP-216 — the bash variant. The `tool_call` carries `content[0].terminalId` and a matching
    /// `_meta.terminal_info`; the update carries both `terminal_output` and `terminal_exit` with
    /// `signal == null`; and an empty output emits `terminal_exit` only.
    #[test]
    fn a_replayed_bash_result_is_a_terminal_and_an_exit() {
        let cwd = AbsCwd::parse("/proj/a").unwrap();
        let out = replay_updates(&[tool_result("sh1", "bash", "hello\n", false, None)], &cwd);
        let json: Vec<Value> = out.iter().map(json_of).collect();
        assert_eq!(json.len(), 2);
        assert_eq!(json[0]["content"][0]["terminalId"], "sh1");
        assert_eq!(json[0]["_meta"]["terminal_info"]["terminal_id"], "sh1");
        assert_eq!(json[0]["_meta"]["terminal_info"]["cwd"], "/proj/a");
        assert_eq!(json[1]["_meta"]["terminal_output"]["data"], "hello\n");
        assert_eq!(json[1]["_meta"]["terminal_exit"]["exit_code"], 0);
        assert!(json[1]["_meta"]["terminal_exit"]["signal"].is_null());
        assert_eq!(json[1]["status"], "completed");

        let empty = replay_updates(&[tool_result("sh2", "bash", "", false, None)], &cwd);
        let empty_json = json_of(&empty[1]);
        assert!(
            empty_json["_meta"].get("terminal_output").is_none(),
            "an empty command emitted a terminal_output: {empty_json}"
        );
        assert_eq!(empty_json["_meta"]["terminal_exit"]["exit_code"], 0);

        let failed = replay_updates(&[tool_result("sh3", "bash", "boom", true, None)], &cwd);
        let failed_json = json_of(&failed[1]);
        assert_eq!(failed_json["status"], "failed");
        assert_eq!(failed_json["_meta"]["terminal_exit"]["exit_code"], 1);
    }

    /// ACP-217's shape, as far as a unit test can reach it: the outcome carries the replay, the
    /// response and the follow-up as three separate fields, so the driver cannot write them in the
    /// wrong order by forgetting which is which. The wire-order assertion itself is a `cyrup-it`
    /// case — it needs a real connection — and is named in this module's report.
    #[test]
    fn the_load_outcome_separates_the_pre_and_post_response_halves() {
        let outcome = LoadOutcome {
            session_id: SessionId::new("s1"),
            replay: vec![SessionUpdate::UserMessageChunk(ContentChunk::new(
                ContentBlock::from("hi".to_string()),
            ))],
            response: LoadSessionResponse::new(),
            follow_up: vec![SessionUpdate::AvailableCommandsUpdate(
                agent_client_protocol::schema::v1::AvailableCommandsUpdate::new(Vec::new()),
            )],
        };
        assert_eq!(
            json_of(&outcome.replay[0])["sessionUpdate"],
            "user_message_chunk"
        );
        assert_eq!(
            json_of(&outcome.follow_up[0])["sessionUpdate"],
            "available_commands_update"
        );
    }

    // ---------------------------------------------------------------------------------------
    // Every entry point answers rather than panicking
    // ---------------------------------------------------------------------------------------

    /// The canary on the never-panic rule. Every one of these returned an "unimplemented" sentinel
    /// before integration; each now reaches its real body and fails for a real reason.
    #[tokio::test]
    async fn every_entry_point_answers_instead_of_panicking() {
        let mgr = SessionManager::new(null_host());
        // `ACP-078` — the setters go through the `Unknown sessionId` gate, and a null host resolves
        // nothing, so this is the miss and NOT the skeleton's `Internal`.
        assert_eq!(
            err_of(
                mgr.set_mode(&SetSessionModeRequest::new(
                    SessionId::new("s1"),
                    agent_client_protocol::schema::v1::SessionModeId::new("off")
                ))
                .await
            ),
            AcpFailure::InvalidParams {
                message: "Unknown sessionId: s1".into()
            }
        );
        assert_eq!(
            err_of(
                mgr.set_config_option(&SetSessionConfigOptionRequest::new(
                    SessionId::new("s1"),
                    agent_client_protocol::schema::v1::SessionConfigId::new("model"),
                    agent_client_protocol::schema::v1::SessionConfigOptionValue::value_id(
                        agent_client_protocol::schema::v1::SessionConfigValueId::new("x")
                    ),
                ))
                .await
            ),
            AcpFailure::InvalidParams {
                message: "Unknown sessionId: s1".into()
            }
        );
        // A `session/new` through a host that builds nothing is an error response, not a panic and
        // not a fabricated session (`ACP-057`'s second assertion).
        assert!(matches!(
            mgr.new_session(&NewSessionRequest::new("/tmp/project"), None)
                .await,
            Err(AcpFailure::Internal { .. })
        ));
        // And the cwd guard runs before the host is ever asked.
        assert_eq!(
            err_of(
                mgr.new_session(&NewSessionRequest::new("relative/dir"), None)
                    .await
            ),
            AcpFailure::InvalidParams {
                message: "cwd must be an absolute path: relative/dir".into()
            }
        );
    }
}
