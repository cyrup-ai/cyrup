//! UM-1 byte-diff guard: every built-in tool's model-facing surface — its `parameters()` JSON
//! Schema AND its `description()` / `prompt_snippet()` / `prompt_guidelines()` — must equal Pi's
//! BYTE-FOR-BYTE.
//!
//! The metadata half (gap 04, TOOL-001/TOOL-003) is asserted through `Arc<dyn cyrup_core::Tool>`
//! ON PURPOSE: the registry stores `Arc<dyn Tool>` (`registry.rs:35,50-66`) and the agent
//! (`cyrup-agent/src/agent.rs`) + prompt builder read the vtable, so a metadata impl that is not
//! reachable through the `Tool` vtable is invisible to the model. Before the TOOL-001 fix the
//! strings lived on a standalone `ToolMeta` trait and every one of these assertions failed with
//! `""` / `None` / `[]`.
//!
//! The `PI_*` constants below are the EXACT output of `JSON.stringify(<schema>)` for each tool's
//! TypeBox `Type.Object(...)`, captured by running Pi's real schema definitions
//! (`read.ts:20-24`, `bash.ts:24-27`, `grep.ts:24-36`, `find.ts:20-26`, `ls.ts:14-17`,
//! `write.ts:14-17`, `edit.ts:33-53`) under Node with the `typebox` package vendored in the Pi
//! repo. They are ground truth, not a paraphrase.
//!
//! The audit (gap 04, UM-1) found the prior hand-written schemas diverged on: paraphrased
//! descriptions, `type:"integer"` (Pi: `"number"`), added `minimum`, and `additionalProperties`
//! present on 6 tools. That last one was under-corrected: it was left standing on `edit` (both
//! levels) on the belief that `edit` alone opts in. It does not — `Type.Object(props, {})`
//! (edit.ts:41,52) passes an EMPTY options object, so NO built-in emits the keyword. Comparing
//! parsed `serde_json::Value`s is the
//! correct JSON byte-diff: two JSON documents are equal iff identical content, independent of the
//! provider's key-ordering at serialize time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_core::Tool;
use crate::config::{BashOpts, FindOpts, GrepOpts, LsOpts, ReadOpts, WriteOpts};
use crate::ops::local::LocalFs;
use crate::ops::{Backend, FsOps, ProcOps, ShellConfig};
use crate::tools::{BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool, WriteTool};
use crate::FileMutationLocks;
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
const PI_EDIT: &str = r#"{"type":"object","required":["path","edits"],"properties":{"path":{"type":"string","description":"Path to the file to edit (relative or absolute)"},"edits":{"type":"array","items":{"type":"object","required":["oldText","newText"],"properties":{"oldText":{"type":"string","description":"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},"newText":{"type":"string","description":"Replacement text for this targeted edit."}}},"description":"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."}}}"#;

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

// --- Ground-truth Pi tool metadata (verbatim, template literals evaluated) ---------------------
//
// Each `description` below is Pi's string with its `${…}` interpolations resolved against Pi's own
// constants: read/bash DEFAULT_MAX_LINES=2000 & DEFAULT_MAX_BYTES/1024=50; grep DEFAULT_LIMIT=100 &
// GREP_MAX_LINE_LENGTH=500; find DEFAULT_LIMIT=1000; ls DEFAULT_LIMIT=500. Written one-per-line so
// a diff against the `.ts` source is a plain string compare.

// read.ts:212-214
const PI_READ_DESCRIPTION: &str = "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to 2000 lines or 50KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.";
const PI_READ_SNIPPET: &str = "Read file contents";
const PI_READ_GUIDELINES: &[&str] = &["Use read to examine files instead of cat or sed."];

// write.ts:189-192
const PI_WRITE_DESCRIPTION: &str = "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.";
const PI_WRITE_SNIPPET: &str = "Create or overwrite files";
const PI_WRITE_GUIDELINES: &[&str] = &["Use write only for new files or complete rewrites."];

// edit.ts:295-304
const PI_EDIT_DESCRIPTION: &str = "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.";
const PI_EDIT_SNIPPET: &str = "Make precise file edits with exact text replacement, including multiple disjoint edits in one call";
const PI_EDIT_GUIDELINES: &[&str] = &[
    "Use edit for precise changes (edits[].oldText must match exactly)",
    "When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls",
    "Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
    "Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.",
];

// v0.84.1 bash.ts:45-48,332-334. `promptGuidelines` is present whenever `exposeSessionEnvironment`
// is on, and bash.ts:327 defaults that flag to TRUE (`options?.exposeSessionEnvironment ?? true`) —
// so the DEFAULT `BashOpts` must carry the guideline. Renamed `PI_*` -> `CYRUP_*` because that is
// the family cyrup's `resolveSpawnContext` port actually publishes (TOOL-008).
//
// The leading "You can " is v0.84.1 `bash.ts:47` verbatim: v0.83.0 `bash.ts:330` said
// "Inspect PI_* environment variables for current model and session details." and v0.84.0 softened
// the imperative to a statement of availability. Version lag, not a port bug.
const PI_BASH_GUIDELINES: &[&str] =
    &["You can inspect CYRUP_* environment variables for current model and session details."];
const PI_BASH_DESCRIPTION: &str = "Execute a bash command in the current working directory. Returns stdout and stderr. Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.";
const PI_BASH_SNIPPET: &str = "Execute bash commands (ls, grep, find, etc.)";

// grep.ts:131-132 — Pi declares no `promptGuidelines`.
const PI_GREP_DESCRIPTION: &str = "Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to 100 matches or 50KB (whichever is hit first). Long lines are truncated to 500 chars.";
const PI_GREP_SNIPPET: &str = "Search file contents for patterns (respects .gitignore)";

// find.ts:117-118 — Pi declares no `promptGuidelines`.
const PI_FIND_DESCRIPTION: &str = "Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to 1000 results or 50KB (whichever is hit first).";
const PI_FIND_SNIPPET: &str = "Find files by glob pattern (respects .gitignore)";

// ls.ts:103-104 — Pi declares no `promptGuidelines`.
const PI_LS_DESCRIPTION: &str = "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. Includes dotfiles. Output is truncated to 500 entries or 50KB (whichever is hit first).";
const PI_LS_SNIPPET: &str = "List directory contents";

/// Assert a tool's full model-facing metadata THROUGH the `Tool` vtable (the only surface the
/// registry, the agent's `ToolDef` build and the system-prompt builder can reach).
fn assert_meta(
    tool: Arc<dyn Tool>,
    name: &str,
    description: &str,
    snippet: &str,
    guidelines: &[&str],
) {
    assert_eq!(tool.name(), name, "tool name");
    // TOOL-045 — pi sets `label` EXPLICITLY on every built-in `ToolDefinition`, immediately after
    // `name`, and for all seven the two strings are equal: `read.ts:210-211` @v0.83.0,
    // `bash.ts:325-326`, `edit.ts:293-294`, `write.ts:187-188`, `grep.ts:129-130`,
    // `find.ts:115-116`, `ls.ts:101-102`. Asserted as `Some(name)`, NOT as "`None` is fine because
    // the runtime falls back to the name": the fallback and an explicit declaration are only
    // indistinguishable while every label happens to equal its name, and this assertion is what
    // makes the seven declarations data. RED for all seven before the fix (`label()` was the
    // trait default `None`, `cyrup-core/src/tool.rs:102-104`).
    assert_eq!(tool.label(), Some(name), "{name} label() diverges from Pi's ToolDefinition.label");
    assert_eq!(tool.description(), description, "{name} description() diverges from Pi");
    assert_eq!(
        tool.prompt_snippet(),
        Some(snippet),
        "{name} prompt_snippet() diverges from Pi promptSnippet"
    );
    assert_eq!(
        tool.prompt_guidelines(),
        guidelines,
        "{name} prompt_guidelines() diverges from Pi promptGuidelines"
    );
}

/// TOOL-001 + TOOL-003: every built-in ships Pi's verbatim `description`, `promptSnippet` and
/// `promptGuidelines` on the `cyrup_core::Tool` vtable. Fails for all seven before the fix
/// (`description()` returned the trait default `""`, `prompt_snippet()` `None`).
#[test]
fn all_seven_tool_metadata_match_pi_verbatim() {
    assert_meta(
        Arc::new(ReadTool::new(fs(), cwd(), ReadOpts::default())),
        "read",
        PI_READ_DESCRIPTION,
        PI_READ_SNIPPET,
        PI_READ_GUIDELINES,
    );
    assert_meta(
        Arc::new(WriteTool::new(fs(), locks(), cwd(), WriteOpts)),
        "write",
        PI_WRITE_DESCRIPTION,
        PI_WRITE_SNIPPET,
        PI_WRITE_GUIDELINES,
    );
    assert_meta(
        Arc::new(EditTool::new(fs(), locks(), cwd(), Default::default())),
        "edit",
        PI_EDIT_DESCRIPTION,
        PI_EDIT_SNIPPET,
        PI_EDIT_GUIDELINES,
    );
    assert_meta(
        Arc::new(BashTool::new(proc(), ShellConfig::detect(), cwd(), BashOpts::default())),
        "bash",
        PI_BASH_DESCRIPTION,
        PI_BASH_SNIPPET,
        PI_BASH_GUIDELINES,
    );
    assert_meta(
        Arc::new(GrepTool::new(fs(), cwd(), GrepOpts::default())),
        "grep",
        PI_GREP_DESCRIPTION,
        PI_GREP_SNIPPET,
        &[],
    );
    assert_meta(
        Arc::new(FindTool::new(fs(), cwd(), FindOpts::default())),
        "find",
        PI_FIND_DESCRIPTION,
        PI_FIND_SNIPPET,
        &[],
    );
    assert_meta(
        Arc::new(LsTool::new(fs(), cwd(), LsOpts::default())),
        "ls",
        PI_LS_DESCRIPTION,
        PI_LS_SNIPPET,
        &[],
    );
}

/// G41 MIRROR: the v0.84.0 softening applied to `bash` ONLY.
///
/// `git diff v0.83.0..v0.84.1 -- packages/coding-agent/src/core/tools/` touches all seven tool
/// files, but the only model-facing STRING it changes is bash's guideline. `read.ts:28` and
/// `write.ts:21` at v0.84.1 still carry the unprefixed `"Use read to examine files instead of cat
/// or sed."` / `"Use write only for new files or complete rewrites."`, and all four `edit.ts:58-61`
/// guidelines are byte-identical to v0.83.0 — the v0.84.0 change hoisted them into exported consts
/// without rewording them. A find-and-replace that softened every guideline would be over-broad
/// and is caught here.
#[test]
fn only_bash_got_the_v0_84_softening() {
    for (name, tool) in [
        ("read", Arc::new(ReadTool::new(fs(), cwd(), ReadOpts::default())) as Arc<dyn Tool>),
        ("write", Arc::new(WriteTool::new(fs(), locks(), cwd(), WriteOpts))),
        ("edit", Arc::new(EditTool::new(fs(), locks(), cwd(), Default::default()))),
    ] {
        for g in tool.prompt_guidelines() {
            assert!(
                !g.starts_with("You can "),
                "{name}: pi v0.84.1 softened bash's guideline only; this one must stay verbatim: {g}"
            );
        }
    }

    let bash = BashTool::new(proc(), ShellConfig::detect(), cwd(), BashOpts::default());
    assert!(bash.prompt_guidelines()[0].starts_with("You can inspect "));
}

/// The registry hands out `Arc<dyn Tool>`; assert the metadata survives that erasure for the whole
/// built-in set, so a future tool added without a description is caught here rather than in a
/// session. This is the exact path `agent.rs` uses to build `cyrup_provider::ToolDef`.
#[test]
fn registry_erased_tools_all_carry_a_description_and_snippet() {
    let registry = crate::ToolRegistry::with_builtins(
        cwd(),
        Backend::default(),
        crate::ToolsOptions::default(),
    );
    let tools = registry.visible(&crate::Availability::All);
    assert_eq!(tools.len(), crate::BUILTIN_NAMES.len(), "all built-ins visible");
    for t in &tools {
        assert!(!t.description().is_empty(), "{}: empty description() reaches the model", t.name());
        assert!(
            t.prompt_snippet().is_some_and(|s| !s.is_empty()),
            "{}: no prompt_snippet() — omitted from the system prompt's Available tools",
            t.name()
        );
    }
}

/// Pin the three specific divergences the audit named, so a regression is legible even if the
/// verbatim-equality test above is later edited.
#[test]
fn schema_scalar_types_are_number_no_minimum_no_extra_additional_properties() {
    let read = ReadTool::new(fs(), cwd(), ReadOpts::default());
    let p = read.parameters();
    assert_eq!(p["properties"]["offset"]["type"], "number"); // Pi: number, NOT integer
    assert!(p["properties"]["offset"].get("minimum").is_none()); // Pi adds no minimum
    assert!(p.get("additionalProperties").is_none()); // Pi never emits it — on ANY built-in

    let ls = LsTool::new(fs(), cwd(), LsOpts::default());
    // ls has NO required key at all (both properties optional) — TypeBox omits empty `required`.
    assert!(ls.parameters().get("required").is_none());

    // `edit` was long believed to be the one built-in that emits `additionalProperties:false`,
    // on the strength of its two-argument `Type.Object(props, {})` calls (edit.ts:33-53). That
    // second argument is an EMPTY options object, and TypeBox spreads only what it is handed, so
    // neither the top object nor the nested edit object carries the keyword. Re-derived by
    // evaluating both literals under typebox 1.3.7 (pi's `package.json` pin at v0.83.0) and 1.1.38
    // (the copy vendored in the pi checkout) — identical output, no `additionalProperties` at
    // either level. The keyword is not inert on the wire: `parameters()` is copied verbatim into
    // the model-facing schema (`cyrup-agent/src/agent.rs:813`) and forwarded untouched by the
    // OpenAI Chat Completions and Google adapters, so pinning its ABSENCE at both levels is what
    // keeps the legacy flat `{path, oldText, newText}` shape (`EditTool::prepare_arguments`,
    // pi `prepareEditArguments` edit.ts:94-118) emittable by a schema-constrained model.
    let edit = EditTool::new(fs(), locks(), cwd(), Default::default());
    assert!(edit.parameters().get("additionalProperties").is_none());
    assert!(edit.parameters()["properties"]["edits"]["items"]
        .get("additionalProperties")
        .is_none());
}
