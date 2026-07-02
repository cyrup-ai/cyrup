//! Chain-file discovery (func-SA §5.1 R-SA-015; arch-SA §6.2.2).
//!
//! A "chain" is a saved, named `RunnerStep` sequence, authored either as `<name>.chain.md`
//! (frontmatter grammar, matching agent `.md` files) or `<name>.chain.json` (plain
//! `serde_json`). Within a single directory's recursive scan, `.chain.json` MUST take
//! precedence over a `.chain.md` file of the *same name*, and that precedence check is
//! explicit — never derived from alphabetical scan order (R-SA-015). Across different scan
//! *scopes* (e.g. user directory vs. project directory), same-named chains are never collapsed
//! into one entry: both survive, each tagged with its own [`AgentSource`], and disambiguation is
//! deferred to the consumer (`discovery/mod.rs`'s four-scope orchestration, R-SA-001, which is
//! not owned by this file).
//!
//! A malformed chain file produces a non-fatal [`ChainDiscoveryDiagnostic`] — neither the abort
//! reserved for malformed `subagents.*` settings, nor the silent per-file skip reserved for
//! malformed agent frontmatter (R-SA-009's three-way throw/silent-skip/diagnostic distinction).
//!
//! # Deferred: full `RunnerStep` field population
//!
//! [`crate::spawn::chain_graph::RunnerStep`] is, as of this file, a temporary placeholder unit
//! struct (`spawn/chain_graph.rs`'s own header) standing in for the real `SingleStep |
//! ParallelGroup | DynamicGroup` discriminated union that a later phase of this crate's build-out
//! owns (arch-SA §2.2 Phase 3, `spawn/chain_graph.rs`). That real type will carry a `Deserialize`
//! impl once it lands. Until then, this module parses each chain file's `steps` array/block only
//! far enough to determine **how many** steps it declares and **in what order**, materializing one
//! placeholder [`RunnerStep`](crate::spawn::chain_graph::RunnerStep) value per parsed step object
//! — preserving `steps.len()` and step ordering faithfully (both are asserted by this module's own
//! tests) without inventing a shape for fields that belong to that later phase. When
//! `chain_graph.rs` lands its real `RunnerStep` with `Deserialize`, `parse_chain_json_steps`/
//! `parse_chain_md_steps` below are the sole call sites that need updating to deserialize full
//! step content instead of counting objects — the directory-scan/precedence algorithm itself
//! (this file's actual R-SA-015 deliverable) does not change.
//!
//! # Deferred: full agent-frontmatter grammar
//!
//! `discovery/frontmatter.rs` (arch-SA §2.2's hand-rolled YAML-subset parser, §6.2.3) is owned by
//! a separate concurrently-authored file and is not yet present. `.chain.md` uses "the same
//! frontmatter grammar" as agent `.md` files (arch-SA §4.1), so once `frontmatter.rs` lands, the
//! minimal local `extract_frontmatter_block` helper below should be replaced with a call into
//! that shared parser rather than maintained as a second implementation. Until then this module
//! implements only the narrow subset it actually needs (flat `key: value` pairs plus one
//! indented-block value for `steps`) so `.chain.md` discovery is real and testable now rather
//! than blocked on that other file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::{AgentSource, ChainDefinition, ChainDiscoveryDiagnostic};
use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};

/// File extension suffix recognized for JSON-format chain files (higher precedence, R-SA-015).
const CHAIN_JSON_SUFFIX: &str = ".chain.json";

/// File extension suffix recognized for frontmatter-format chain files (lower precedence,
/// R-SA-015, on a same-name collision within one directory scan).
const CHAIN_MD_SUFFIX: &str = ".chain.md";

/// Directory segment reserved for skill bundling (R-SA-007's exclusion, mirrored here so a
/// `SKILL.md`-bearing subtree is never descended into while scanning for chain files — chain
/// files are never legitimately nested under a skill bundle).
const SKILLS_DIR_SEGMENT: &str = "skills";

/// One directory scan's outcome: the winning [`ChainDefinition`] per name (after applying
/// R-SA-015's `.chain.json` > `.chain.md` same-name precedence) plus any non-fatal parse
/// diagnostics collected along the way.
#[derive(Debug, Default)]
pub struct ChainScanResult {
    pub chains: Vec<ChainDefinition>,
    pub diagnostics: Vec<ChainDiscoveryDiagnostic>,
}

/// Scan `root` recursively for chain files (`*.chain.json` / `*.chain.md`), tag every discovered
/// [`ChainDefinition`] with `source`, and apply R-SA-015's same-directory-scan, same-name
/// `.chain.json` > `.chain.md` precedence.
///
/// Traversal order follows R-SA-004's alphabetical-by-filename, depth-first convention (mirrored
/// from `cyrup-resources`' own `scan_skill_dir` walk, `crates/cyrup-resources/src/discovery.rs`):
/// each directory's children are sorted before iteration, and a subdirectory is fully descended
/// before its next sibling is visited. That traversal order is irrelevant to *this* function's own
/// precedence outcome (format precedence is checked explicitly per R-SA-015, never derived from
/// scan order) but is kept consistent with the rest of this crate's discovery code for a
/// deterministic, reproducible `diagnostics` ordering.
///
/// A directory that does not exist (or is not readable) yields an empty, non-error result — an
/// absent scope directory is not itself a malformed-chain-file condition.
pub fn scan_chain_dir(root: &Path, source: AgentSource) -> ChainScanResult {
    let mut by_name: HashMap<String, ChainCandidate> = HashMap::new();
    let mut diagnostics = Vec::new();
    walk_dir(root, source, &mut by_name, &mut diagnostics);

    let mut chains: Vec<ChainDefinition> = by_name.into_values().map(|c| c.definition).collect();
    // Deterministic output order (by name) independent of `HashMap` iteration order, so repeated
    // calls over the same on-disk state are stable for callers/tests.
    chains.sort_by(|a, b| a.name.cmp(&b.name));
    diagnostics.sort_by(|a, b| a.file_path.cmp(&b.file_path));

    ChainScanResult {
        chains,
        diagnostics,
    }
}

/// Scan multiple `(root, source)` scopes and concatenate their [`scan_chain_dir`] results into one
/// flat list, **without** merging across scopes (R-SA-015's cross-scope retention rule). Two
/// scopes that each define a chain named `"release"` both appear in the returned `Vec`, each
/// tagged with its own [`AgentSource`] — disambiguation among same-named cross-scope chains is
/// deliberately left to the consumer (management/execution-time lookup in a later phase's
/// `discovery/mod.rs`, not this file).
pub fn scan_chain_scopes(scopes: &[(PathBuf, AgentSource)]) -> ChainScanResult {
    let mut chains = Vec::new();
    let mut diagnostics = Vec::new();
    for (root, source) in scopes {
        let mut scoped = scan_chain_dir(root, *source);
        chains.append(&mut scoped.chains);
        diagnostics.append(&mut scoped.diagnostics);
    }
    ChainScanResult {
        chains,
        diagnostics,
    }
}

/// One in-progress per-directory-scan candidate: the currently-winning [`ChainDefinition`] for a
/// given chain name, plus which format produced it (needed to enforce R-SA-015's explicit format
/// check — a later `.chain.md` insertion MUST NOT overwrite an earlier `.chain.json` insertion of
/// the same name, regardless of alphabetical scan order).
struct ChainCandidate {
    definition: ChainDefinition,
    format: ChainFileFormat,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChainFileFormat {
    Json,
    Md,
}

/// Depth-first, alphabetical-by-filename directory walk (R-SA-004's traversal convention). Chain
/// files are recognized by their double-suffix (`.chain.json`/`.chain.md`) rather than a bare
/// extension, since a plain `.json`/`.md` file in the same directory is not a chain file.
fn walk_dir(
    dir: &Path,
    source: AgentSource,
    by_name: &mut HashMap<String, ChainCandidate>,
    diagnostics: &mut Vec<ChainDiscoveryDiagnostic>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();

    for path in children {
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if path.is_dir() {
            // R-SA-007: never descend into a directory segment reserved for skill bundling —
            // `SKILL.md`/skill-bundle subtrees are never a legitimate chain-file location.
            if file_name == SKILLS_DIR_SEGMENT {
                continue;
            }
            walk_dir(&path, source, by_name, diagnostics);
            continue;
        }

        let format = if file_name.ends_with(CHAIN_JSON_SUFFIX) {
            ChainFileFormat::Json
        } else if file_name.ends_with(CHAIN_MD_SUFFIX) {
            ChainFileFormat::Md
        } else {
            continue;
        };

        let name = chain_name_from_file_name(file_name, format);
        if name.is_empty() {
            diagnostics.push(ChainDiscoveryDiagnostic {
                file_path: path,
                source,
                message: "chain file name has no name component before its suffix".to_string(),
            });
            continue;
        }

        let parsed = match format {
            ChainFileFormat::Json => parse_chain_json(&path, &name, source),
            ChainFileFormat::Md => parse_chain_md(&path, &name, source),
        };

        let definition = match parsed {
            Ok(def) => def,
            Err(message) => {
                diagnostics.push(ChainDiscoveryDiagnostic {
                    file_path: path,
                    source,
                    message,
                });
                continue;
            }
        };

        insert_with_format_precedence(by_name, name, definition, format);
    }
}

/// R-SA-015's core rule, isolated to one call site: `.chain.json` always wins over a same-name
/// `.chain.md` **regardless of which was scanned first**. Concretely:
/// - No existing candidate for this name: insert unconditionally.
/// - Existing candidate is `Json` and the new one is `Md`: keep the existing `Json` candidate.
/// - Existing candidate is `Md` and the new one is `Json`: replace with the new `Json` candidate.
/// - Both `Json` or both `Md` (two same-format, same-name files in different subdirectories of one
///   scan root): last-scanned-wins, consistent with this crate's directory-walk-order convention
///   elsewhere (R-SA-004) — format precedence, not scan order, is what R-SA-015 constrains.
fn insert_with_format_precedence(
    by_name: &mut HashMap<String, ChainCandidate>,
    name: String,
    definition: ChainDefinition,
    format: ChainFileFormat,
) {
    match by_name.get(&name) {
        Some(existing)
            if existing.format == ChainFileFormat::Json && format == ChainFileFormat::Md =>
        {
            // A `.chain.json` candidate is already installed for this name: a same-name
            // `.chain.md` MUST NOT overwrite it, no matter the scan order that produced either.
        }
        _ => {
            by_name.insert(name, ChainCandidate { definition, format });
        }
    }
}

/// Strip the recognized chain-file suffix to recover the chain's name, e.g.
/// `"release.chain.json"` -> `"release"`.
fn chain_name_from_file_name(file_name: &str, format: ChainFileFormat) -> String {
    let suffix = match format {
        ChainFileFormat::Json => CHAIN_JSON_SUFFIX,
        ChainFileFormat::Md => CHAIN_MD_SUFFIX,
    };
    file_name
        .strip_suffix(suffix)
        .unwrap_or(file_name)
        .to_string()
}

// -------------------------------------------------------------------------------------------
// `.chain.json` parsing
// -------------------------------------------------------------------------------------------

/// Parse a `.chain.json` file into a [`ChainDefinition`]. Plain `serde_json` (arch-SA §4.1) — no
/// frontmatter delimiters involved for this format.
fn parse_chain_json(
    path: &Path,
    file_name_key: &str,
    source: AgentSource,
) -> Result<ChainDefinition, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read chain file: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("invalid JSON in chain file: {e}"))?;

    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| file_name_key.to_string());
    let description = value
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let steps = parse_chain_json_steps(&value)?;

    Ok(ChainDefinition {
        name,
        description,
        source,
        file_path: path.to_path_buf(),
        steps,
    })
}

/// Extract the `steps` array from a parsed `.chain.json` document, deserializing each element as
/// a real, tagged [`RunnerStep`] (`spawn::chain_graph::RunnerStep` now has its real
/// `SingleStep | ParallelGroup | DynamicGroup` shape and a `Deserialize` impl — this file's
/// former "Deferred: full `RunnerStep` field population" placeholder era, see the module-level
/// doc's own note on this being the anticipated follow-up). An element that is present but does
/// not deserialize as a well-formed tagged `RunnerStep` is treated as a per-element malformed-step
/// condition and fails the whole chain file (surfaced as a [`ChainDiscoveryDiagnostic`] by this
/// file's caller, `walk_dir`) rather than silently degrading to a placeholder step, so a
/// genuinely malformed step is never silently misrepresented as a valid (if empty) one.
fn parse_chain_json_steps(value: &serde_json::Value) -> Result<Vec<RunnerStep>, String> {
    match value.get("steps") {
        None => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                serde_json::from_value::<RunnerStep>(item.clone())
                    .map_err(|e| format!("chain step[{index}] is not a valid RunnerStep: {e}"))
            })
            .collect(),
        Some(_) => Err("chain file's \"steps\" field must be an array".to_string()),
    }
}

// -------------------------------------------------------------------------------------------
// `.chain.md` parsing (frontmatter grammar)
// -------------------------------------------------------------------------------------------

/// Parse a `.chain.md` file into a [`ChainDefinition`]. Uses the same frontmatter shape as agent
/// `.md` files (arch-SA §4.1): a leading `---`-delimited block of flat `key: value` pairs, with
/// `steps` supplied either as a fenced JSON array value or as a nested indented block whose lines
/// are themselves flat `key: value` step stubs (one blank-line-or-`-`-prefixed entry per step).
/// See this file's module header for why this is a narrow, self-contained subset rather than a
/// call into `discovery/frontmatter.rs` (not yet present).
fn parse_chain_md(
    path: &Path,
    file_name_key: &str,
    source: AgentSource,
) -> Result<ChainDefinition, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read chain file: {e}"))?;
    let (frontmatter, _body) = extract_frontmatter_block(&raw)
        .ok_or_else(|| "chain file is missing a --- delimited frontmatter block".to_string())?;

    let fields = parse_flat_frontmatter(frontmatter);

    let name = fields
        .get("name")
        .cloned()
        .unwrap_or_else(|| file_name_key.to_string());
    let description = fields.get("description").cloned().unwrap_or_default();
    let steps = parse_chain_md_steps(frontmatter)?;

    Ok(ChainDefinition {
        name,
        description,
        source,
        file_path: path.to_path_buf(),
        steps,
    })
}

/// Split a `.md` file's content into its `---`-delimited frontmatter block and trailing body.
/// Returns `None` if the file does not open with a frontmatter delimiter line.
fn extract_frontmatter_block(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    let frontmatter = &rest[..end];
    let after_delim = &rest[end + 4..];
    let body = after_delim
        .strip_prefix('\n')
        .or_else(|| after_delim.strip_prefix("\r\n"))
        .unwrap_or(after_delim);
    Some((frontmatter, body))
}

/// Parse the flat (non-indented) `key: value` lines of a frontmatter block into a map. Indented
/// continuation lines (used by the `steps:` block, handled separately by
/// [`parse_chain_md_steps`]) are skipped here rather than folded into a value, matching arch-SA
/// §6.2.3's "flat key: value plus one level of block-indent values" grammar description.
fn parse_flat_frontmatter(frontmatter: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for line in frontmatter.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        fields.insert(key.to_string(), value.to_string());
    }
    fields
}

/// Extract the `steps:` block from a `.chain.md` frontmatter section. Two shapes are accepted:
/// - `steps: [...]` — a single-line inline JSON array, parsed with `serde_json`.
/// - `steps:` followed by indented lines, each indented line beginning a new step stub — the step
///   *count* is the number of top-level indented entries (lines starting with `- ` at the first
///   indent level), matching this file's "count and order only" deferral (see module header).
///
/// Absence of a `steps:` key yields an empty step list rather than an error — a chain file with no
/// steps yet (e.g. scaffolded but not filled in) is not itself malformed.
fn parse_chain_md_steps(frontmatter: &str) -> Result<Vec<RunnerStep>, String> {
    let mut lines = frontmatter.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, inline_value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "steps" {
            continue;
        }

        let inline_value = inline_value.trim();
        if !inline_value.is_empty() {
            // Inline `steps: [...]` form.
            let value: serde_json::Value = serde_json::from_str(inline_value)
                .map_err(|e| format!("invalid inline \"steps\" JSON array: {e}"))?;
            return parse_chain_json_steps(&serde_json::json!({ "steps": value }));
        }

        // Block form: count indented top-level list-entry lines (`  - ...`) that follow, until a
        // non-indented (or end-of-frontmatter) line is reached. Each stub line (e.g.
        // `- agent: reviewer`) carries no structured per-step JSON this narrow frontmatter
        // subset attempts to parse (see this file's module header's "Deferred: full
        // agent-frontmatter grammar" note), so each is materialized as a minimal, valid
        // placeholder `RunnerStep::SingleStep` — preserving count and order only, exactly
        // mirroring `discovery::management::placeholder_runner_step`'s identical convention for
        // the same "preserve count, not content" need.
        let mut count = 0usize;
        while let Some(next) = lines.peek() {
            if !(next.starts_with(' ') || next.starts_with('\t')) {
                break;
            }
            let trimmed = next.trim_start();
            if trimmed.starts_with("- ") || trimmed == "-" {
                count += 1;
            }
            lines.next();
        }
        return Ok((0..count).map(|_| placeholder_runner_step()).collect());
    }
    Ok(Vec::new())
}

/// Build one minimal, valid placeholder [`RunnerStep::SingleStep`] — used only to preserve step
/// *count* for `.chain.md`'s block-form `steps:` stub lines, which carry no structured per-step
/// content this narrow frontmatter subset attempts to parse (see [`parse_chain_md_steps`]'s own
/// doc comment). Every field left at its "no override" default so this placeholder carries no
/// spurious behavior if ever (mis)dispatched directly. Mirrors `discovery::management`'s own
/// private `placeholder_runner_step` helper's identical convention for the same "preserve count,
/// not content" need in that sibling module (each module keeps its own copy rather than sharing
/// one `pub` helper, since neither module's placeholder-construction need is itself part of
/// either module's own public contract).
fn placeholder_runner_step() -> RunnerStep {
    RunnerStep::SingleStep(SingleStepSpec {
        agent: String::new(),
        task: String::new(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_mode: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write fixture chain file");
        path
    }

    /// Each element is a real, minimal, well-formed tagged `RunnerStep::SingleStep` JSON object
    /// (`spawn::chain_graph::RunnerStep` now has a real `Deserialize` impl this module's own
    /// `parse_chain_json_steps` deserializes against, no longer a placeholder-counting shim) —
    /// `i` is folded into `agent`/`task` purely so distinct steps are trivially distinguishable
    /// in any test assertion that wants to, without every fixture step needing to be identical.
    fn sample_json(steps: usize) -> String {
        let steps_json: Vec<String> = (0..steps)
            .map(|i| {
                format!(
                    "{{\"kind\":\"singleStep\",\"agent\":\"agent-{i}\",\"task\":\"task-{i}\"}}"
                )
            })
            .collect();
        format!(
            "{{\"name\":\"release\",\"description\":\"release chain\",\"steps\":[{}]}}",
            steps_json.join(",")
        )
    }

    fn sample_md(steps: usize) -> String {
        let mut steps_block = String::new();
        for _ in 0..steps {
            steps_block.push_str("  - agent: reviewer\n");
        }
        format!(
            "---\nname: release\ndescription: release chain (md)\nsteps:\n{steps_block}---\nBody text.\n"
        )
    }

    #[test]
    fn chain_json_wins_over_chain_md_at_same_scope_and_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "release.chain.md", &sample_md(2));
        write(tmp.path(), "release.chain.json", &sample_json(3));

        let result = scan_chain_dir(tmp.path(), AgentSource::Project);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.chains.len(), 1);
        let chain = &result.chains[0];
        assert_eq!(chain.name, "release");
        assert_eq!(chain.file_path, tmp.path().join("release.chain.json"));
        assert_eq!(chain.steps.len(), 3);
    }

    #[test]
    fn chain_json_wins_over_chain_md_regardless_of_alphabetical_scan_order() {
        // ".chain.json" < ".chain.md" is NOT the point being tested here; both filenames sort
        // the same either way ("release.chain.json" < "release.chain.md" alphabetically), so
        // additionally verify the reverse-insertion code path directly via the internal helper,
        // proving the precedence is an explicit format check rather than an artifact of
        // alphabetical order (R-SA-015).
        let mut by_name: HashMap<String, ChainCandidate> = HashMap::new();
        let json_def = ChainDefinition {
            name: "release".to_string(),
            description: "json".to_string(),
            source: AgentSource::User,
            file_path: PathBuf::from("/scope/release.chain.json"),
            steps: vec![placeholder_runner_step(), placeholder_runner_step()],
        };
        let md_def = ChainDefinition {
            name: "release".to_string(),
            description: "md".to_string(),
            source: AgentSource::User,
            file_path: PathBuf::from("/scope/release.chain.md"),
            steps: vec![placeholder_runner_step()],
        };

        // Insert Json first, then Md: Md must not win.
        insert_with_format_precedence(
            &mut by_name,
            "release".to_string(),
            json_def,
            ChainFileFormat::Json,
        );
        insert_with_format_precedence(
            &mut by_name,
            "release".to_string(),
            md_def,
            ChainFileFormat::Md,
        );
        let winner = &by_name.get("release").expect("winner present").definition;
        assert_eq!(winner.file_path, PathBuf::from("/scope/release.chain.json"));

        // Insert Md first, then Json: Json must win (overwrite).
        let mut by_name2: HashMap<String, ChainCandidate> = HashMap::new();
        insert_with_format_precedence(
            &mut by_name2,
            "release".to_string(),
            ChainDefinition {
                name: "release".to_string(),
                description: "md".to_string(),
                source: AgentSource::User,
                file_path: PathBuf::from("/scope/release.chain.md"),
                steps: vec![placeholder_runner_step()],
            },
            ChainFileFormat::Md,
        );
        insert_with_format_precedence(
            &mut by_name2,
            "release".to_string(),
            ChainDefinition {
                name: "release".to_string(),
                description: "json".to_string(),
                source: AgentSource::User,
                file_path: PathBuf::from("/scope/release.chain.json"),
                steps: vec![placeholder_runner_step(), placeholder_runner_step()],
            },
            ChainFileFormat::Json,
        );
        let winner2 = &by_name2.get("release").expect("winner present").definition;
        assert_eq!(
            winner2.file_path,
            PathBuf::from("/scope/release.chain.json")
        );
    }

    #[test]
    fn both_formats_parse_into_the_same_chain_definition_shape() {
        let json_tmp = tempfile::tempdir().expect("tempdir");
        write(json_tmp.path(), "release.chain.json", &sample_json(2));
        let json_result = scan_chain_dir(json_tmp.path(), AgentSource::User);
        assert!(
            json_result.diagnostics.is_empty(),
            "{:?}",
            json_result.diagnostics
        );
        assert_eq!(json_result.chains.len(), 1);
        let from_json = &json_result.chains[0];

        let md_tmp = tempfile::tempdir().expect("tempdir");
        write(md_tmp.path(), "release.chain.md", &sample_md(2));
        let md_result = scan_chain_dir(md_tmp.path(), AgentSource::User);
        assert!(
            md_result.diagnostics.is_empty(),
            "{:?}",
            md_result.diagnostics
        );
        assert_eq!(md_result.chains.len(), 1);
        let from_md = &md_result.chains[0];

        assert_eq!(from_json.name, from_md.name);
        assert_eq!(from_json.steps.len(), from_md.steps.len());
        assert_eq!(from_json.source, from_md.source);
    }

    #[test]
    fn cross_scope_same_name_chains_are_both_retained_not_merged() {
        let user_tmp = tempfile::tempdir().expect("tempdir");
        write(user_tmp.path(), "release.chain.json", &sample_json(1));
        let project_tmp = tempfile::tempdir().expect("tempdir");
        write(project_tmp.path(), "release.chain.json", &sample_json(4));

        let result = scan_chain_scopes(&[
            (user_tmp.path().to_path_buf(), AgentSource::User),
            (project_tmp.path().to_path_buf(), AgentSource::Project),
        ]);

        assert_eq!(result.chains.len(), 2);
        assert!(result.chains.iter().any(|c| c.source == AgentSource::User));
        assert!(
            result
                .chains
                .iter()
                .any(|c| c.source == AgentSource::Project)
        );
    }

    #[test]
    fn malformed_json_chain_file_produces_diagnostic_not_abort() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "broken.chain.json", "{ not valid json ");
        write(tmp.path(), "release.chain.json", &sample_json(1));

        let result = scan_chain_dir(tmp.path(), AgentSource::Project);
        assert_eq!(result.diagnostics.len(), 1);
        assert!(
            result.diagnostics[0]
                .file_path
                .ends_with("broken.chain.json")
        );
        // Sibling file discovery continues unaffected.
        assert_eq!(result.chains.len(), 1);
        assert_eq!(result.chains[0].name, "release");
    }

    #[test]
    fn malformed_md_chain_file_missing_frontmatter_produces_diagnostic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "broken.chain.md",
            "no frontmatter here at all\n",
        );

        let result = scan_chain_dir(tmp.path(), AgentSource::User);
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.chains.is_empty());
    }

    #[test]
    fn nested_subdirectories_are_scanned_recursively() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir nested");
        // No "name" field in the body: the discovered chain's name falls back to the file stem
        // ("deep"), keeping this test focused on recursive traversal rather than on the
        // separately-covered name-field-vs-filename precedence (see
        // `chain_name_defaults_to_file_stem_when_name_field_absent`).
        write(
            &nested,
            "deep.chain.json",
            "{\"steps\":[{\"kind\":\"singleStep\",\"agent\":\"a\",\"task\":\"t\"}]}",
        );

        let result = scan_chain_dir(tmp.path(), AgentSource::Project);
        assert_eq!(result.chains.len(), 1);
        assert_eq!(result.chains[0].name, "deep");
        assert_eq!(result.chains[0].steps.len(), 1);
    }

    #[test]
    fn skill_bundle_subdirectory_is_excluded_from_chain_discovery() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).expect("mkdir skills");
        write(&skills_dir, "not-a-chain.chain.json", &sample_json(1));

        let result = scan_chain_dir(tmp.path(), AgentSource::Project);
        assert!(result.chains.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn missing_scan_root_yields_empty_result_not_error() {
        let result = scan_chain_dir(Path::new("/does/not/exist/at/all"), AgentSource::User);
        assert!(result.chains.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn absent_steps_key_yields_empty_step_list_not_malformed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "no-steps.chain.json",
            "{\"name\":\"no-steps\",\"description\":\"d\"}",
        );
        let result = scan_chain_dir(tmp.path(), AgentSource::User);
        assert!(result.diagnostics.is_empty());
        assert_eq!(result.chains.len(), 1);
        assert!(result.chains[0].steps.is_empty());
    }

    #[test]
    fn chain_name_defaults_to_file_stem_when_name_field_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "unnamed.chain.json", "{\"steps\":[]}");
        let result = scan_chain_dir(tmp.path(), AgentSource::User);
        assert_eq!(result.chains.len(), 1);
        assert_eq!(result.chains[0].name, "unnamed");
    }
}
