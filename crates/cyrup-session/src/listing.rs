//! Session listing & selection (arch-04 §4.3/§6.6, R-04-015/018/019). Streaming header/text scan;
//! selection by full path or unique uuid prefix.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cyrup_core::{Content, Message, SessionId};

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

/// All sessions for a cwd, newest first (R-04-015).
pub fn list(layout: &SessionLayout) -> Vec<SessionInfo> {
    let mut out = scan_dir(&layout.dir());
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

/// All sessions under the root across projects, newest first (R-04-015).
pub fn list_all(root: &SessionsRoot) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root.path()) {
        for dir in rd.flatten() {
            let p = dir.path();
            if p.is_dir() {
                out.extend(scan_dir(&p));
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

fn scan_dir(dir: &Path) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(info) = scan_file(&path) {
                out.push(info);
            }
        }
    }
    out
}

fn scan_file(path: &Path) -> Option<SessionInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let header: SessionHeader = serde_json::from_str(lines.next()?).ok()?;
    if header.kind != "session" {
        return None;
    }

    let meta = std::fs::metadata(path).ok();
    let created = meta.as_ref().and_then(|m| m.created().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
    let modified = meta.and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);

    let mut message_count = 0usize;
    let mut first_message = String::new();
    let mut all_text = String::new();
    let mut name: Option<String> = None;

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Entry>(line) else {
            continue;
        };
        match entry {
            Entry::Known(KnownEntry::Message { message, .. }) => {
                message_count += 1;
                let t = message_text(&message);
                if !t.is_empty() {
                    if first_message.is_empty() {
                        first_message = t.clone();
                    }
                    all_text.push_str(&t);
                    all_text.push('\n');
                }
            }
            Entry::Known(KnownEntry::SessionInfo { name: n @ Some(_), .. }) => {
                name = n;
            }
            _ => {}
        }
    }

    Some(SessionInfo {
        path: path.to_path_buf(),
        id: header.id,
        cwd: header.cwd,
        name,
        parent_session_path: header.parent_session.map(PathBuf::from),
        created,
        modified,
        message_count,
        first_message,
        all_messages_text: all_text,
    })
}

fn message_text(m: &Message) -> String {
    let blocks = match m {
        Message::User { content, .. } | Message::ToolResult { content, .. } => content,
        Message::Assistant(a) => &a.content,
    };
    let mut s = String::new();
    for b in blocks {
        if let Content::Text { text, .. } = b {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(text);
        }
    }
    s
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
