//! The session header (line 1 of the JSONL file) and schema versioning (arch-04 §3.1/§4.2).

use cyrup_core::SessionId;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Current schema version cyrup writes (v3 = unified-roles). Files at v1/v2 are auto-migrated on
/// load (R-04-004).
pub const CURRENT_VERSION: u32 = 3;

/// Line 1 of the file: session metadata. NOT a tree node (no `id`/`parentId` of its own here —
/// `id` is the session uuid, not an entry id).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeader {
    /// Always `"session"`; the file-level discriminant separating the header from tree entries.
    #[serde(rename = "type")]
    pub kind: String,
    /// v1 files omit this (deserializes to `None` → treated as v1). cyrup always writes
    /// `CURRENT_VERSION` (R-04-004).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Session uuid (v7, time-sortable). The one field besides `type` that is load-bearing on
    /// READ: Pi validates the header with `header.type !== "session" || typeof header.id !==
    /// "string"` and nothing else (`loadEntriesFromFile`, `session-manager.ts:548-552`;
    /// `parseSessionHeaderCandidate`, `:566`). [`SessionId`] is `#[serde(transparent)]` over
    /// `Arc<str>`, so a non-string `id` fails here exactly as it fails there.
    pub id: SessionId,
    /// RFC3339 creation timestamp. **Read-tolerant** — see [`de_string_or_empty`].
    #[serde(default, deserialize_with = "de_string_or_empty")]
    pub timestamp: String,
    /// The working directory the session belongs to. **Read-tolerant** — see
    /// [`de_string_or_empty`].
    #[serde(default, deserialize_with = "de_string_or_empty")]
    pub cwd: String,
    /// Source file path if this session was forked/cloned (R-04-020/021).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

/// Read a header string field the way Pi's *runtime* does: absent or non-string becomes `""`.
///
/// Pi's `interface SessionHeader` (`session-manager.ts:32-39`) declares `timestamp: string` and
/// `cwd: string` as required, but that is a **compile-time** TypeScript type over a bare
/// `JSON.parse` — the same mechanism gap `SESS-001`/`SESS-027` already established for message
/// content. Every runtime reader in `session-manager.ts` therefore re-checks them by hand, and none
/// treats a miss as an error:
///
/// - `getSessionHeaderCwd` (`:625-628`) is
///   `` const cwd = (header as { cwd?: unknown }).cwd; return typeof cwd === "string" ? cwd : undefined; ``,
///   and `static open` (`:1546`) folds that through `?? process.cwd()`.
/// - `buildSessionInfo` (`:739`, `:742`) is
///   `` const cwd = typeof header.cwd === "string" ? header.cwd : ""; `` and
///   `` const headerTime = typeof header.timestamp === "string" ? new Date(...).getTime() : NaN; ``,
///   with the `NaN` arm falling back to the file's mtime (`:743-748`).
/// - the two header validators (`:548-552`, `:566`) test `type` and `id` **only**.
///
/// cyrup declared both fields as plain required `String`s, so serde rejected the whole line, the
/// candidate was demoted to "not a session header", and a file pi opens normally became a hard
/// `SessionError::NotASession` from `SessionManager::open` and vanished from every listing. Empty
/// is the right landing value on both paths: `open_with_cwd` already treats an empty header cwd as
/// "no header cwd" and falls through to the process cwd, and `listing::scan_file` already falls
/// back to the file mtime for an unparseable timestamp.
fn de_string_or_empty<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => s,
        _ => String::new(),
    })
}

impl SessionHeader {
    /// Construct a fresh header at the current version.
    pub fn new(id: SessionId, cwd: impl Into<String>, timestamp: impl Into<String>) -> Self {
        Self {
            kind: "session".to_string(),
            version: Some(CURRENT_VERSION),
            id,
            timestamp: timestamp.into(),
            cwd: cwd.into(),
            parent_session: None,
        }
    }

    /// Effective version (a missing field means the legacy linear v1 format).
    pub fn effective_version(&self) -> u32 {
        self.version.unwrap_or(1)
    }
}
