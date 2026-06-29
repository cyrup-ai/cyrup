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
    /// Scan an assistant message's tool calls, tracking the `read`/`write`/`edit` tools' `path`
    /// argument. Pi `extractFileOpsFromMessage` (`utils.ts:38-55`) matches the tool name with an
    /// EXACT switch (not a substring) and reads the path ONLY from `args.path`, so an unrelated tool
    /// (e.g. `multiedit`) or a differently-named path arg is not tracked.
    pub fn absorb_message(&mut self, msg: &Message) {
        if let Message::Assistant(a) = msg {
            for c in &a.content {
                if let Content::ToolCall(tc) = c
                    && let Some(path) = extract_path(&tc.arguments)
                {
                    match tc.name.as_str() {
                        "read" => {
                            self.read.insert(path);
                        }
                        "write" => {
                            self.written.insert(path);
                        }
                        "edit" => {
                            self.edited.insert(path);
                        }
                        _ => {}
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

/// Pull the filesystem path from a tool call's `args.path`. Pi reads ONLY `args.path`
/// (`utils.ts:43`), so no alternate key (`file_path`, `absolutePath`, …) is consulted.
fn extract_path(args: &serde_json::Map<String, Value>) -> Option<String> {
    args.get("path").and_then(Value::as_str).map(str::to_string)
}

/// `<read-files>`/`<modified-files>` blocks for the non-empty sections only, prefixed with `\n\n`,
/// or `""` when both are empty (Pi `formatFileOperations`, `utils.ts:72-82`). Each section is emitted
/// ONLY when it has entries — never an empty `<read-files></read-files>` block.
pub fn format_file_operations(read: &[String], modified: &[String]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !read.is_empty() {
        sections.push(format!("<read-files>\n{}\n</read-files>", read.join("\n")));
    }
    if !modified.is_empty() {
        sections.push(format!("<modified-files>\n{}\n</modified-files>", modified.join("\n")));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}
