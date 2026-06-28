//! The session header (line 1 of the JSONL file) and schema versioning (arch-04 §3.1/§4.2).

use cyrup_core::SessionId;
use serde::{Deserialize, Serialize};

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
    /// Session uuid (v7, time-sortable).
    pub id: SessionId,
    /// RFC3339 creation timestamp.
    pub timestamp: String,
    /// The working directory the session belongs to.
    pub cwd: String,
    /// Source file path if this session was forked/cloned (R-04-020/021).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
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
