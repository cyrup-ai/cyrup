//! Session listing & selection (arch-04 §4.3/§6.6, R-04-015/018/019). Streaming header/text scan;
//! selection by full path or unique uuid prefix.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cyrup_core::{Content, Message, SessionId};
use serde_json::Value;

use crate::agent_message::AgentMessage;
use crate::entry::{Entry, KnownEntry};
use crate::error::SessionError;
use crate::header::SessionHeader;
use crate::layout::{SessionLayout, SessionsRoot};

/// Lightweight summary of a session file for `/resume`-style listing.
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub path: PathBuf,
    pub id: SessionId,
    pub cwd: String,
    pub name: Option<String>,
    pub parent_session_path: Option<PathBuf>,
    pub created: SystemTime,
    pub modified: SystemTime,
    pub message_count: usize,
    pub first_message: String,
    pub all_messages_text: String,
}

/// How a session is selected on the CLI (R-04-019).
#[derive(Clone, Debug)]
pub enum SessionSelector {
    Path(PathBuf),
    /// Full uuid or a unique prefix.
    Uuid(String),
}

/// Progress callback for listing: invoked `(loaded, total)` after each session file is processed —
/// Pi `SessionListProgress` (`session-manager.ts:670`), used by the TUI session selector.
pub type SessionListProgress<'a> = dyn FnMut(usize, usize) + 'a;

/// All sessions for a cwd, newest first (R-04-015).
pub fn list(layout: &SessionLayout) -> Vec<SessionInfo> {
    list_in_dir(&layout.dir(), None, None)
}

/// List a directory's sessions newest-first, optionally filtering by `cwd_filter` and reporting
/// `(loaded, total)` progress — Pi `SessionManager.list(cwd, sessionDir, onProgress)`
/// (`session-manager.ts:1507-1516`): a `cwd_filter` (set when a custom/shared `sessionDir` is not
/// the cwd-default) keeps only sessions whose header cwd matches, so a shared directory only shows
/// the current project. Listing is synchronous (Pi's bounded-concurrency is an arch choice); the
/// `(loaded, total)` affordance is preserved.
pub fn list_in_dir(
    dir: &Path,
    cwd_filter: Option<&Path>,
    on_progress: Option<&mut SessionListProgress>,
) -> Vec<SessionInfo> {
    let paths = collect_paths(dir);
    let total = paths.len();
    let mut out = load_infos(&paths, cwd_filter, total, on_progress);
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

/// All sessions under the root across projects, newest first (R-04-015).
pub fn list_all(root: &SessionsRoot) -> Vec<SessionInfo> {
    list_all_with_progress(root, None)
}

/// All sessions across every project directory under the root, with total-first `(loaded, total)`
/// progress — Pi `SessionManager.listAll(onProgress)` (`session-manager.ts:1522-1580`): file counts
/// are summed across project dirs first so progress totals are accurate, then every file is loaded.
pub fn list_all_with_progress(
    root: &SessionsRoot,
    on_progress: Option<&mut SessionListProgress>,
) -> Vec<SessionInfo> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root.path()) {
        for dir in rd.flatten() {
            let p = dir.path();
            if p.is_dir() {
                paths.extend(collect_paths(&p));
            }
        }
    }
    let total = paths.len();
    let mut out = load_infos(&paths, None, total, on_progress);
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

/// List a single custom session directory newest-first with progress (no cwd filter) — Pi's
/// `SessionManager.listAll(sessionDir, onProgress)` overload (`session-manager.ts:1528-1535`).
pub fn list_all_in_dir(
    dir: &Path,
    on_progress: Option<&mut SessionListProgress>,
) -> Vec<SessionInfo> {
    list_in_dir(dir, None, on_progress)
}

/// Newest `*.jsonl` in `dir` whose header parses, optionally restricted to sessions whose header
/// cwd matches `cwd_filter` — Pi `findMostRecentSession(sessionDir, cwd?)`
/// (`session-manager.ts:539-559`). Files without a readable header are skipped (Pi requires a
/// non-null header); selection is by mtime, newest wins.
pub fn newest_session(dir: &Path, cwd_filter: Option<&Path>) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for path in collect_paths(dir) {
        let header = match read_header(&path) {
            Some(h) => h,
            None => continue,
        };
        if let Some(c) = cwd_filter
            && !session_cwd_matches(&header.cwd, c)
        {
            continue;
        }
        let modified = match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if best.as_ref().is_none_or(|(t, _)| modified >= *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Pi `sessionCwdMatches` (`session-manager.ts:534-536`): a non-empty session cwd that resolves
/// equal to the requested cwd.
fn session_cwd_matches(session_cwd: &str, resolved_cwd: &Path) -> bool {
    !session_cwd.is_empty() && Path::new(session_cwd) == resolved_cwd
}

/// The `.jsonl` file paths directly in `dir` (unsorted); empty if the dir is unreadable.
fn collect_paths(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(path);
            }
        }
    }
    out
}

/// Parse `paths` into [`SessionInfo`]s, applying an optional cwd filter and invoking `on_progress`
/// `(loaded, total)` after EVERY file (matching Pi, which counts each attempted file regardless of
/// parse success — `session-manager.ts:695-698,731-734`).
fn load_infos(
    paths: &[PathBuf],
    cwd_filter: Option<&Path>,
    total: usize,
    mut on_progress: Option<&mut SessionListProgress>,
) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    let mut loaded = 0usize;
    for p in paths {
        let info = scan_file(p);
        loaded += 1;
        if let Some(cb) = on_progress.as_mut() {
            cb(loaded, total);
        }
        if let Some(info) = info
            && cwd_filter.is_none_or(|c| session_cwd_matches(&info.cwd, c))
        {
            out.push(info);
        }
    }
    out
}

/// Read and parse just the header line of a session file (Pi `readSessionHeader`).
fn read_header(path: &Path) -> Option<SessionHeader> {
    let text = std::fs::read_to_string(path).ok()?;
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    let header: SessionHeader = serde_json::from_str(first).ok()?;
    if header.kind == "session" {
        Some(header)
    } else {
        None
    }
}

fn scan_file(path: &Path) -> Option<SessionInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let header: SessionHeader = serde_json::from_str(lines.next()?).ok()?;
    if header.kind != "session" {
        return None;
    }

    let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());

    let mut message_count = 0usize;
    let mut first_message = String::new();
    let mut all_messages: Vec<String> = Vec::new();
    let mut name: Option<String> = None;
    // Latest user/assistant message activity time (ms), Pi `getMessageActivityTime`.
    let mut last_activity_ms: Option<i64> = None;

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Entry>(line) else {
            continue;
        };
        // Latest session_info wins, including explicit clears (Pi `session-manager.ts:616-618`).
        if let Entry::Known(KnownEntry::SessionInfo { name: n, .. }) = &entry {
            name = n.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            continue;
        }
        // Pi's scan is UNTYPED: `parseSessionEntryLine` is a bare `JSON.parse`
        // (`session-manager.ts:503-511`), so `if (entry.type !== "message") continue; messageCount++`
        // (`:717-718`) counts on the TAG alone — before any role filter and before any body
        // validation. cyrup's `Entry` deserializer demotes a known tag whose body does not fit the
        // typed `AgentMessage` to `Entry::Unknown` (`entry.rs:277-279`) — a legacy pre-v3
        // `hookMessage` role (`migrate.rs:29-32`), an unknown content-block type, a missing
        // `timestamp` — so that shape is matched here too rather than silently skipped.
        let (typed, raw, entry_ts) = match &entry {
            Entry::Known(KnownEntry::Message { message, base }) => {
                (Some(message), None, base.timestamp.as_str())
            }
            Entry::Unknown(v) if v.get("type").and_then(Value::as_str) == Some("message") => (
                None,
                Some(v),
                v.get("timestamp").and_then(Value::as_str).unwrap_or_default(),
            ),
            _ => continue,
        };
        message_count += 1;

        // Only core user/assistant messages contribute activity time + text (Pi:566-588,628-638);
        // an untyped body is read field-by-field exactly as Pi's `isMessageWithContent` +
        // `extractTextContent` read the raw JSON (`:658-671`).
        let (role_text, is_user, msg_ts) = match (typed, raw) {
            (Some(AgentMessage::Core(Message::User { content, timestamp })), _) => {
                (core_text(content), true, *timestamp)
            }
            (Some(AgentMessage::Core(Message::Assistant(a))), _) => {
                (core_text(&a.content), false, a.timestamp)
            }
            (None, Some(v)) => match raw_core_message(v) {
                Some(parts) => parts,
                None => continue,
            },
            _ => continue,
        };

        let activity_ms = if msg_ts != 0 { msg_ts } else { rfc3339_to_ms(entry_ts).unwrap_or(0) };
        last_activity_ms = Some(last_activity_ms.unwrap_or(0).max(activity_ms));

        if role_text.is_empty() {
            continue;
        }
        all_messages.push(role_text.clone());
        if first_message.is_empty() && is_user {
            first_message = role_text;
        }
    }

    let created =
        rfc3339_to_systemtime(&header.timestamp).unwrap_or(SystemTime::UNIX_EPOCH);
    // modified = latest message activity, else header time, else file mtime (Pi:645-651).
    let modified = match last_activity_ms {
        Some(ms) if ms > 0 => ms_to_systemtime(ms),
        _ => rfc3339_to_systemtime(&header.timestamp)
            .or(mtime)
            .unwrap_or(SystemTime::UNIX_EPOCH),
    };

    Some(SessionInfo {
        path: path.to_path_buf(),
        id: header.id,
        cwd: header.cwd,
        name,
        parent_session_path: header.parent_session.map(PathBuf::from),
        created,
        modified,
        message_count,
        first_message: if first_message.is_empty() {
            "(no messages)".to_string()
        } else {
            first_message
        },
        all_messages_text: all_messages.join(" "),
    })
}

/// The `(text, is_user, message-timestamp-ms)` of a raw `message` entry whose body cyrup could not
/// type ([`Entry::Unknown`]) — Pi's listing never types it either, it just reads the fields:
/// `isMessageWithContent` requires a string `role` and a present `content`
/// (`session-manager.ts:658-660`), the role filter keeps only `user`/`assistant` (`:726-727`),
/// `extractTextContent` takes a bare string content as is or joins its `type:"text"` blocks with
/// `" "` (`:662-671`), and `getMessageActivityTime` reads a numeric `message.timestamp`
/// (`:673-684`). `None` = not a core-role message, i.e. contributes neither text nor activity time
/// (a pre-v3 `hookMessage`, a `bashExecution`, …) — it is still COUNTED by the caller.
fn raw_core_message(entry: &Value) -> Option<(String, bool, i64)> {
    let msg = entry.get("message")?;
    let is_user = match msg.get("role").and_then(Value::as_str)? {
        "user" => true,
        "assistant" => false,
        _ => return None,
    };
    let text = match msg.get("content")? {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    Some((text, is_user, msg.get("timestamp").and_then(Value::as_i64).unwrap_or(0)))
}

/// Join the text blocks of a core message with `" "` (Pi `extractTextContent`,
/// `session-manager.ts:565-574`).
fn core_text(blocks: &[Content]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse an RFC3339 timestamp to unix milliseconds.
fn rfc3339_to_ms(s: &str) -> Option<i64> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
}

/// Parse an RFC3339 timestamp to a `SystemTime`.
fn rfc3339_to_systemtime(s: &str) -> Option<SystemTime> {
    rfc3339_to_ms(s).map(ms_to_systemtime)
}

/// Convert unix milliseconds to a `SystemTime` (clamped at the epoch for negatives).
fn ms_to_systemtime(ms: i64) -> SystemTime {
    if ms >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_millis(ms as u64)
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_millis((-ms) as u64)
    }
}

/// Resolve a selector to a concrete session file path (R-04-019).
pub fn resolve(sel: &SessionSelector, layout: &SessionLayout) -> Result<PathBuf, SessionError> {
    match sel {
        SessionSelector::Path(p) if p.exists() => Ok(p.clone()),
        SessionSelector::Path(p) => {
            Err(SessionError::NotFound { what: p.display().to_string() })
        }
        SessionSelector::Uuid(prefix) => {
            let matches: Vec<PathBuf> = scan_session_paths(&layout.dir())
                .into_iter()
                .filter(|f| uuid_of(f).is_some_and(|u| u.starts_with(prefix.as_str())))
                .collect();
            match matches.as_slice() {
                [one] => Ok(one.clone()),
                [] => Err(SessionError::NotFound { what: prefix.clone() }),
                _ => Err(SessionError::AmbiguousSelector {
                    prefix: prefix.clone(),
                    n: matches.len(),
                }),
            }
        }
    }
}

fn scan_session_paths(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(path);
            }
        }
    }
    out
}

/// Extract the uuid component from a `<timestamp>_<uuid>.jsonl` filename.
fn uuid_of(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    stem.rsplit_once('_').map(|(_, u)| u.to_string()).or_else(|| Some(stem.to_string()))
}
