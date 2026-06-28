//! Cumulative read/modified file tracking (arch-05 §3.8, R-05-015) and the `<read-files>` /
//! `<modified-files>` machine-readable summary blocks (R-05-013).

use std::collections::BTreeSet;

use cyrup_core::{Content, Message};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default `details` payload stored on a compaction/branch-summary entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// Accumulates file operations across the messages being summarized plus any previous
/// compaction/branch-summary `details` (cumulative; R-05-015).
#[derive(Clone, Debug, Default)]
pub struct FileOps {
    pub read: BTreeSet<String>,
    pub written: BTreeSet<String>,
    pub edited: BTreeSet<String>,
}

impl FileOps {
    /// Scan an assistant message's tool calls for read/write/edit + a `path`-like argument.
    pub fn absorb_message(&mut self, msg: &Message) {
        if let Message::Assistant(a) = msg {
            for c in &a.content {
                if let Content::ToolCall(tc) = c {
                    let name = tc.name.to_lowercase();
                    if let Some(path) = extract_path(&tc.arguments) {
                        if name.contains("read") {
                            self.read.insert(path);
                        } else if name.contains("write") {
                            self.written.insert(path);
                        } else if name.contains("edit") {
                            self.edited.insert(path);
                        }
                    }
                }
            }
        }
    }

    /// Seed from a previous compaction/branch-summary `details` JSON (cumulative). Parses
    /// defensively: a non-conforming `details` simply contributes no files (R-00-009).
    pub fn absorb_prev_details(&mut self, details: &Value) {
        if let Some(arr) = details.get("readFiles").and_then(Value::as_array) {
            for v in arr {
                if let Some(s) = v.as_str() {
                    self.read.insert(s.to_string());
                }
            }
        }
        if let Some(arr) = details.get("modifiedFiles").and_then(Value::as_array) {
            for v in arr {
                if let Some(s) = v.as_str() {
                    // A previously-modified file stays modified (it is not demoted to read-only).
                    self.edited.insert(s.to_string());
                }
            }
        }
    }

    /// `modifiedFiles = edited ∪ written` (sorted); `readFiles = read \ modified` (sorted).
    pub fn compute_lists(&self) -> (Vec<String>, Vec<String>) {
        let mut modified: BTreeSet<String> = self.edited.clone();
        modified.extend(self.written.iter().cloned());
        let read: Vec<String> =
            self.read.iter().filter(|p| !modified.contains(*p)).cloned().collect();
        (read, modified.into_iter().collect())
    }

    /// Build the default `CompactionDetails` from the computed lists.
    pub fn to_details(&self) -> CompactionDetails {
        let (read_files, modified_files) = self.compute_lists();
        CompactionDetails { read_files, modified_files }
    }
}

/// Pull a filesystem path from common tool-call argument keys.
fn extract_path(args: &Value) -> Option<String> {
    for key in ["path", "file_path", "filePath", "absolutePath", "absolute_path"] {
        if let Some(s) = args.get(key).and_then(Value::as_str) {
            return Some(s.to_string());
        }
    }
    None
}

/// `"\n\n<read-files>…</read-files>\n\n<modified-files>…</modified-files>"`, or `""` when empty
/// (R-05-013).
pub fn format_file_operations(read: &[String], modified: &[String]) -> String {
    if read.is_empty() && modified.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("\n\n<read-files>\n");
    out.push_str(&read.join("\n"));
    out.push_str("\n</read-files>\n\n<modified-files>\n");
    out.push_str(&modified.join("\n"));
    out.push_str("\n</modified-files>");
    out
}
