//! UM-1 byte-diff guard: every built-in tool's model-facing `parameters()` JSON Schema must equal
//! Pi's TypeBox-emitted `input_schema` BYTE-FOR-BYTE (as JSON documents).
//!
//! The `PI_*` constants below are the EXACT output of `JSON.stringify(<schema>)` for each tool's
//! TypeBox `Type.Object(...)`, captured by running Pi's real schema definitions
//! (`read.ts:20-24`, `bash.ts:24-27`, `grep.ts:24-36`, `find.ts:20-26`, `ls.ts:14-17`,
//! `write.ts:14-17`, `edit.ts:33-53`) under Node with the `typebox` package vendored in the Pi
//! repo. They are ground truth, not a paraphrase.
//!
//! The audit (gap 04, UM-1) found the prior hand-written schemas diverged on: paraphrased
//! descriptions, `type:"integer"` (Pi: `"number"`), added `minimum`, and `additionalProperties`
//! present on 6 tools (Pi sets it on `edit` ONLY). Comparing parsed `serde_json::Value`s is the
//! correct JSON byte-diff: two JSON documents are equal iff identical content, independent of the
//! provider's key-ordering at serialize time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_core::Tool;
use cyrup_tools::config::{BashOpts, FindOpts, GrepOpts, LsOpts, ReadOpts, WriteOpts};
use cyrup_tools::ops::local::LocalFs;
use cyrup_tools::ops::{Backend, FsOps, ProcOps, ShellConfig};
use cyrup_tools::tools::{BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool, WriteTool};
use cyrup_tools::FileMutationLocks;
use std::path::PathBuf;
use std::sync::Arc;

fn fs() -> Arc<dyn FsOps> {
    Arc::new(LocalFs)
}
fn proc() -> Arc<dyn ProcOps> {
    Backend::default().proc
}
fn cwd() -> PathBuf {
    PathBuf::from("/work")
}
fn locks() -> Arc<FileMutationLocks> {
    Arc::new(FileMutationLocks::new())
}

// --- Ground-truth Pi TypeBox JSON (verbatim `JSON.stringify` output) ---------------------------

const PI_READ: &str = r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string","description":"Path to the file to read (relative or absolute)"},"offset":{"type":"number","description":"Line number to start reading from (1-indexed)"},"limit":{"type":"number","description":"Maximum number of lines to read"}}}"#;
const PI_BASH: &str = r#"{"type":"object","required":["command"],"properties":{"command":{"type":"string","description":"Bash command to execute"},"timeout":{"type":"number","description":"Timeout in seconds (optional, no default timeout)"}}}"#;
const PI_GREP: &str = r#"{"type":"object","required":["pattern"],"properties":{"pattern":{"type":"string","description":"Search pattern (regex or literal string)"},"path":{"type":"string","description":"Directory or file to search (default: current directory)"},"glob":{"type":"string","description":"Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'"},"ignoreCase":{"type":"boolean","description":"Case-insensitive search (default: false)"},"literal":{"type":"boolean","description":"Treat pattern as literal string instead of regex (default: false)"},"context":{"type":"number","description":"Number of lines to show before and after each match (default: 0)"},"limit":{"type":"number","description":"Maximum number of matches to return (default: 100)"}}}"#;
const PI_FIND: &str = r#"{"type":"object","required":["pattern"],"properties":{"pattern":{"type":"string","description":"Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'"},"path":{"type":"string","description":"Directory to search in (default: current directory)"},"limit":{"type":"number","description":"Maximum number of results (default: 1000)"}}}"#;
const PI_LS: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"Directory to list (default: current directory)"},"limit":{"type":"number","description":"Maximum number of entries to return (default: 500)"}}}"#;
const PI_WRITE: &str = r#"{"type":"object","required":["path","content"],"properties":{"path":{"type":"string","description":"Path to the file to write (relative or absolute)"},"content":{"type":"string","description":"Content to write to the file"}}}"#;
const PI_EDIT: &str = r#"{"type":"object","required":["path","edits"],"properties":{"path":{"type":"string","description":"Path to the file to edit (relative or absolute)"},"edits":{"type":"array","items":{"type":"object","required":["oldText","newText"],"properties":{"oldText":{"type":"string","description":"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},"newText":{"type":"string","description":"Replacement text for this targeted edit."}},"additionalProperties":false},"description":"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."}},"additionalProperties":false}"#;

fn assert_schema(tool_name: &str, got: &serde_json::Value, pi_json: &str) {
    let want: serde_json::Value = serde_json::from_str(pi_json).expect("Pi JSON parses");
    assert_eq!(
        *got, want,
        "{tool_name} parameters() schema diverges from Pi's TypeBox input_schema"
    );
}

#[test]
fn all_seven_tool_schemas_match_pi_typebox_bytes() {
    let read = ReadTool::new(fs(), cwd(), ReadOpts::default());
    assert_schema("read", read.parameters(), PI_READ);

    let bash = BashTool::new(proc(), ShellConfig::detect(), cwd(), BashOpts::default());
    assert_schema("bash", bash.parameters(), PI_BASH);

    let grep = GrepTool::new(fs(), cwd(), GrepOpts::default());
    assert_schema("grep", grep.parameters(), PI_GREP);

    let find = FindTool::new(fs(), cwd(), FindOpts::default());
    assert_schema("find", find.parameters(), PI_FIND);

    let ls = LsTool::new(fs(), cwd(), LsOpts::default());
    assert_schema("ls", ls.parameters(), PI_LS);

    let write = WriteTool::new(fs(), locks(), cwd(), WriteOpts);
    assert_schema("write", write.parameters(), PI_WRITE);

    let edit = EditTool::new(fs(), locks(), cwd(), Default::default());
    assert_schema("edit", edit.parameters(), PI_EDIT);
}

/// Pin the three specific divergences the audit named, so a regression is legible even if the
/// verbatim-equality test above is later edited.
#[test]
fn schema_scalar_types_are_number_no_minimum_no_extra_additional_properties() {
    let read = ReadTool::new(fs(), cwd(), ReadOpts::default());
    let p = read.parameters();
    assert_eq!(p["properties"]["offset"]["type"], "number"); // Pi: number, NOT integer
    assert!(p["properties"]["offset"].get("minimum").is_none()); // Pi adds no minimum
    assert!(p.get("additionalProperties").is_none()); // Pi sets it on edit ONLY

    let ls = LsTool::new(fs(), cwd(), LsOpts::default());
    // ls has NO required key at all (both properties optional) — TypeBox omits empty `required`.
    assert!(ls.parameters().get("required").is_none());

    let edit = EditTool::new(fs(), locks(), cwd(), Default::default());
    assert_eq!(edit.parameters()["additionalProperties"], false);
    assert_eq!(
        edit.parameters()["properties"]["edits"]["items"]["additionalProperties"],
        false
    );
}
