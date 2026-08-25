//! Chain-file discovery + parsing (func-SA §5.1 R-SA-015; arch-SA §6.2.2), a faithful port of
//! `pi-subagents/src/agents/chain-serializer.ts` (`parseChain`/`parseJsonChain`) plus
//! `agents.ts::loadChainsFromDir`'s directory scan + `.chain.json` > `.chain.md` precedence.
//!
//! A "chain" is a saved, named step sequence authored either as `<name>.chain.md` (the `## <agent>`
//! body-section grammar — the same frontmatter shape as agent `.md` files, then one section per
//! step) or `<name>.chain.json` (a root object with a `chain` array). Both parse into a
//! [`ChainDefinition`] whose `steps` are [`ChainStepConfig`] authoring shapes.
//!
//! # `.chain.md` grammar (`chain-serializer.ts:9-126`)
//!
//! Leading `---`-delimited frontmatter supplies `name`+`description` (both REQUIRED — absence is a
//! parse error, never a silent stem-name fallback) and an optional `package`. The body is split on
//! `^##\s+(.+)$` header lines: each section's config lines (`output`/`phase`/`label`/`as`/
//! `outputSchema`/`outputMode`/`reads`/`model`/`skills`/`progress`) run until the first blank line,
//! and everything after that blank line is the step's task. An inline `outputSchema` value
//! (starting `{`/`[`) is rejected — `.chain.md` `outputSchema` must be a schema-file path.
//!
//! # `.chain.json` grammar (`chain-serializer.ts:128-199`)
//!
//! A root JSON object with a required string `name`, string `description`, and array `chain` (NOT
//! `steps` — reading the wrong root key was the prior invented behavior this port removes). Each
//! `chain[]` element must be an object; each element's `acceptance` (and, for a static-parallel
//! step, each `parallel[]` task's `acceptance`; for a dynamic step, the single `parallel` template
//! object's `acceptance`) is validated via [`validate_acceptance_input`]; the whole `chain` array
//! is then run through [`validate_chain_output_bindings`] (named-output uniqueness, `{outputs.x}`
//! reference resolution, dynamic-fanout shape) exactly as pi does with `{ maxItems: MAX }`.
//!
//! # Precedence + diagnostics (R-SA-015 / R-SA-009)
//!
//! Within one directory scan, a `.chain.json` beats a same-name `.chain.md` regardless of scan
//! order (explicit format check, never derived from alphabetical order). Across scan *scopes*
//! (user vs. project), same-named chains are both retained, each tagged with its own
//! [`AgentSource`]. A malformed chain file produces a non-fatal [`ChainDiscoveryDiagnostic`]
//! (neither the abort reserved for malformed `subagents.*` settings, nor the silent per-file skip
//! reserved for malformed agent frontmatter).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::frontmatter::parse_frontmatter_block;
use super::types::{
    AgentSource, ChainDefinition, ChainDiscoveryDiagnostic, ChainListBinding, ChainOutputBinding,
    ChainStepConfig, OutputMode,
};
use crate::spawn::chain_graph::{
    DynamicGroupSpec, OnEmpty, ParallelGroupSpec, RunnerStep, SingleStepSpec,
};
use super::package_name::normalize_valid_package_name;

/// File extension suffix recognized for JSON-format chain files (higher precedence, R-SA-015).
const CHAIN_JSON_SUFFIX: &str = ".chain.json";

/// File extension suffix recognized for frontmatter-format chain files (lower precedence,
/// R-SA-015, on a same-name collision within one directory scan).
const CHAIN_MD_SUFFIX: &str = ".chain.md";

/// Directory segment reserved for skill bundling (R-SA-007's exclusion, mirrored here so a
/// `SKILL.md`-bearing subtree is never descended into while scanning for chain files — chain
/// files are never legitimately nested under a skill bundle).
const SKILLS_DIR_SEGMENT: &str = "skills";

/// The `maxItems` value pi passes into `validateChainOutputBindings` at chain-parse time
/// (`chain-serializer.ts:204`, `Number.MAX_SAFE_INTEGER`) — parse-time validation never enforces a
/// concrete fan-out ceiling; that is a run-time concern. Kept as the JS safe-integer maximum so a
/// dynamic step's own `maxItems` (if any) is the only ceiling checked here.
const CHAIN_PARSE_MAX_ITEMS: u64 = 9_007_199_254_740_991;

// -------------------------------------------------------------------------------------------
// Directory scan + precedence (agents.ts::loadChainsFromDir)
// -------------------------------------------------------------------------------------------

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
/// `.chain.json` > `.chain.md` precedence (keyed on the *parsed* runtime name, exactly as
/// `agents.ts::loadChainsFromDir` keys its `Map` on `chain.name`).
///
/// A directory that does not exist (or is not readable) yields an empty, non-error result — an
/// absent scope directory is not itself a malformed-chain-file condition.
pub fn scan_chain_dir(root: &Path, source: AgentSource) -> ChainScanResult {
    let mut by_name: HashMap<String, ChainCandidate> = HashMap::new();
    let mut diagnostics = Vec::new();
    walk_dir(root, root, source, &mut by_name, &mut diagnostics);

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
/// deliberately left to the consumer (`discovery/mod.rs`'s four-scope orchestration).
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
    root: &Path,
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
            // pi shares ONE `listFilesRecursive` between the agent walk and the chain walk
            // (`agents.ts:1485` / `:1653`), so the same prune set applies here.
            if super::should_prune_discovery_dir(root, &path, file_name) {
                continue;
            }
            walk_dir(root, &path, source, by_name, diagnostics);
            continue;
        }

        let format = if file_name.ends_with(CHAIN_JSON_SUFFIX) {
            ChainFileFormat::Json
        } else if file_name.ends_with(CHAIN_MD_SUFFIX) {
            ChainFileFormat::Md
        } else {
            continue;
        };

        let parsed = match format {
            ChainFileFormat::Json => parse_chain_json(&path, source),
            ChainFileFormat::Md => parse_chain_md(&path, source),
        };

        match parsed {
            Ok(def) => {
                // Key on the PARSED runtime name (`chain.name`), matching pi's `Map<chain.name>`.
                let name = def.name.clone();
                insert_with_format_precedence(by_name, name, def, format);
            }
            Err(message) => {
                diagnostics.push(ChainDiscoveryDiagnostic {
                    file_path: path,
                    source,
                    message,
                });
            }
        }
    }
}

/// R-SA-015's core rule, isolated to one call site (mirrors `loadChainsFromDir`'s `if (existing &&
/// existing.filePath.endsWith(".chain.json") && filePath.endsWith(".chain.md")) continue`):
/// `.chain.json` always wins over a same-name `.chain.md` **regardless of which was scanned first**.
/// - No existing candidate for this name: insert unconditionally.
/// - Existing is `Json` and the new one is `Md`: keep the existing `Json` candidate.
/// - Otherwise (existing `Md` + new `Json`, or same-format collision): last-scanned-wins.
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

// -------------------------------------------------------------------------------------------
// `.chain.json` parsing (parseJsonChain, chain-serializer.ts:128-199)
// -------------------------------------------------------------------------------------------

/// Parse a `.chain.json` file into a [`ChainDefinition`]. Reads the root `chain` array (erroring on
/// absence), validates per-step acceptance + chain output bindings, and qualifies the runtime name.
fn parse_chain_json(path: &Path, source: AgentSource) -> Result<ChainDefinition, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read chain file: {e}"))?;
    let file_display = path.display();

    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("Invalid JSON chain '{file_display}': {e}"))?;
    let Value::Object(input) = &parsed else {
        return Err(format!("JSON chain '{file_display}' must contain an object root."));
    };

    let name = input
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("JSON chain '{file_display}' must include string name."))?;
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("JSON chain '{file_display}' must include string description."))?;

    let Some(Value::Array(chain)) = input.get("chain") else {
        return Err(format!("JSON chain '{file_display}' must include array chain."));
    };

    for (index, step) in chain.iter().enumerate() {
        let step_no = index + 1;
        if !step.is_object() {
            return Err(format!(
                "JSON chain '{file_display}' step {step_no} must be an object."
            ));
        }
        let acceptance_errors =
            validate_acceptance_input(step.get("acceptance"), &format!("step {step_no} acceptance"));
        if !acceptance_errors.is_empty() {
            return Err(format!(
                "Invalid JSON chain '{file_display}': {}",
                acceptance_errors.join(" ")
            ));
        }
        match step.get("parallel") {
            Some(Value::Array(tasks)) => {
                for (task_index, task) in tasks.iter().enumerate() {
                    if !task.is_object() {
                        continue;
                    }
                    let task_errors = validate_acceptance_input(
                        task.get("acceptance"),
                        &format!(
                            "step {step_no} parallel task {} acceptance",
                            task_index + 1
                        ),
                    );
                    if !task_errors.is_empty() {
                        return Err(format!(
                            "Invalid JSON chain '{file_display}': {}",
                            task_errors.join(" ")
                        ));
                    }
                }
            }
            Some(template) if template.is_object() => {
                let template_errors = validate_acceptance_input(
                    template.get("acceptance"),
                    &format!("step {step_no} dynamic template acceptance"),
                );
                if !template_errors.is_empty() {
                    return Err(format!(
                        "Invalid JSON chain '{file_display}': {}",
                        template_errors.join(" ")
                    ));
                }
            }
            _ => {}
        }
    }

    validate_chain_output_bindings(chain)
        .map_err(|message| format!("Invalid JSON chain '{file_display}': {message}"))?;

    let package_name = parse_chain_package_name(
        input.get("package").and_then(Value::as_str),
        &format!("Chain '{name}' package"),
    )?;

    let mut extra_fields: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in input {
        if key == "name" || key == "package" || key == "description" || key == "chain" {
            continue;
        }
        if let Some(text) = value.as_str() {
            extra_fields.insert(key.clone(), text.to_string());
        }
    }

    let mut steps = Vec::with_capacity(chain.len());
    for step in chain {
        let config: ChainStepConfig = serde_json::from_value(step.clone())
            .map_err(|e| format!("Invalid JSON chain '{file_display}': {e}"))?;
        steps.push(config);
    }

    Ok(ChainDefinition {
        name: build_runtime_name(name, package_name.as_deref()),
        local_name: name.to_string(),
        package_name,
        description: description.to_string(),
        source,
        file_path: path.to_path_buf(),
        steps,
        extra_fields,
    })
}

// -------------------------------------------------------------------------------------------
// `.chain.md` parsing (parseChain, chain-serializer.ts:9-126)
// -------------------------------------------------------------------------------------------

/// Parse a `.chain.md` file into a [`ChainDefinition`]. Uses the shared agent/chain frontmatter
/// grammar for `name`/`description`/`package`, then the `## <agent>` body-section grammar for steps.
fn parse_chain_md(path: &Path, source: AgentSource) -> Result<ChainDefinition, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read chain file: {e}"))?;
    let parsed = parse_frontmatter_block(&raw);

    let name = parsed.get("name").unwrap_or("");
    let description = parsed.get("description").unwrap_or("");
    if name.is_empty() || description.is_empty() {
        return Err("Chain frontmatter must include name and description".to_string());
    }
    let name = name.to_string();
    let description = description.to_string();

    let steps = parse_md_step_sections(&parsed.body)?;

    let package_name =
        parse_chain_package_name(parsed.get("package"), &format!("Chain '{name}' package"))?;

    let mut extra_fields: BTreeMap<String, String> = BTreeMap::new();
    for key in parsed.keys() {
        if key == "name" || key == "package" || key == "description" {
            continue;
        }
        if let Some(value) = parsed.get(key) {
            extra_fields.insert(key.to_string(), value.to_string());
        }
    }

    Ok(ChainDefinition {
        name: build_runtime_name(&name, package_name.as_deref()),
        local_name: name,
        package_name,
        description,
        source,
        file_path: path.to_path_buf(),
        steps,
        extra_fields,
    })
}

/// One `## <agent>` header found in the body, with the byte offsets pi's `matchAll` + slice logic
/// (`chain-serializer.ts:93-104`) needs: the header line's own start (the section's *end* boundary
/// for the preceding step) and the offset immediately after the header line (this section's start).
struct MdHeader {
    agent: String,
    line_start: usize,
    body_start: usize,
}

/// Split a `.chain.md` body into its `## <agent>` sections and parse each into a
/// [`ChainStepConfig`] (`chain-serializer.ts:93-104`). Section bodies run from just after a header
/// line to just before the next header line (or end of body), then are `trim_end`-ed.
fn parse_md_step_sections(body: &str) -> Result<Vec<ChainStepConfig>, String> {
    let mut headers: Vec<MdHeader> = Vec::new();
    let mut offset = 0usize;
    for line in body.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let content = line.strip_suffix('\n').unwrap_or(line);
        if let Some(agent) = match_chain_header(content) {
            headers.push(MdHeader {
                agent,
                line_start,
                body_start: offset,
            });
        }
    }

    let mut steps = Vec::with_capacity(headers.len());
    for (index, header) in headers.iter().enumerate() {
        let section_end = headers
            .get(index + 1)
            .map_or(body.len(), |next| next.line_start);
        let section = body
            .get(header.body_start..section_end)
            .unwrap_or("")
            .trim_end();
        steps.push(parse_step_body(&header.agent, section)?);
    }
    Ok(steps)
}

/// Match one `## <agent>` header line, returning the trimmed agent name (mirrors
/// `^##\s+(.+)[^\S\n]*$`): requires `##` followed by at least one space/tab, then a non-empty
/// remainder. Returns `None` for `###`-style deeper headers, `##`-with-no-space, or an all-blank
/// remainder.
fn match_chain_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix("##")?;
    let after_ws = rest.trim_start_matches([' ', '\t']);
    if after_ws.len() == rest.len() {
        // No whitespace after `##` (e.g. `###` or `##name`): `\s+` requires at least one.
        return None;
    }
    let agent = after_ws.trim();
    if agent.is_empty() {
        return None;
    }
    Some(agent.to_string())
}

/// Parse one section's body into a [`ChainStepConfig`] (`parseStepBody`, `chain-serializer.ts:9-85`):
/// config lines (`key: value`) up to the first blank line, then the remainder (trimmed) as the task.
fn parse_step_body(agent: &str, section_body: &str) -> Result<ChainStepConfig, String> {
    let lines: Vec<&str> = section_body.split('\n').collect();
    let blank_index = lines.iter().position(|line| line.trim().is_empty());

    let config_lines: &[&str] = match blank_index {
        Some(index) => lines.get(..index).unwrap_or(&[]),
        None => &lines,
    };
    let task = match blank_index {
        Some(index) => lines
            .get(index + 1..)
            .map(|rest| rest.join("\n"))
            .unwrap_or_default()
            .trim()
            .to_string(),
        None => String::new(),
    };

    let mut step = ChainStepConfig {
        agent: Some(agent.to_string()),
        task: Some(task),
        ..ChainStepConfig::default()
    };

    for line in config_lines {
        let Some((key, raw_value)) = match_config_line(line) else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        let raw_value = raw_value.trim();

        match key.as_str() {
            "output" => {
                if raw_value == "false" {
                    step.output = Some(ChainOutputBinding::Toggle(false));
                } else if !raw_value.is_empty() {
                    step.output = Some(ChainOutputBinding::Name(raw_value.to_string()));
                }
            }
            "phase" => {
                if !raw_value.is_empty() {
                    step.phase = Some(raw_value.to_string());
                }
            }
            "label" => {
                if !raw_value.is_empty() {
                    step.label = Some(raw_value.to_string());
                }
            }
            "as" => {
                if !raw_value.is_empty() {
                    step.as_ = Some(raw_value.to_string());
                }
            }
            "outputschema" => {
                if raw_value.starts_with('{') || raw_value.starts_with('[') {
                    return Err("Inline outputSchema values are not supported in .chain.md files; use a schema file path.".to_string());
                }
                if !raw_value.is_empty() {
                    step.output_schema = Some(Value::String(raw_value.to_string()));
                }
            }
            "outputmode" => {
                if raw_value == "inline" || raw_value == "file-only" {
                    step.output_mode = Some(raw_value.to_string());
                }
            }
            "reads" => {
                step.reads = Some(parse_list_binding(raw_value));
            }
            "model" => {
                if !raw_value.is_empty() {
                    step.model = Some(raw_value.to_string());
                }
            }
            "skills" => {
                step.skills = Some(parse_list_binding(raw_value));
            }
            "progress" => {
                if raw_value == "true" {
                    step.progress = Some(true);
                } else if raw_value == "false" {
                    step.progress = Some(false);
                }
            }
            _ => {}
        }
    }

    Ok(step)
}

/// pi's `reads`/`skills` config-line parsing: `false` -> disabled; otherwise a comma-separated,
/// trimmed, empty-filtered list (which itself collapses to `false` when nothing remains).
fn parse_list_binding(raw_value: &str) -> ChainListBinding {
    if raw_value == "false" {
        return ChainListBinding::Toggle(false);
    }
    let list: Vec<String> = raw_value
        .split(',')
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    if list.is_empty() {
        ChainListBinding::Toggle(false)
    } else {
        ChainListBinding::List(list)
    }
}

/// Match one frontmatter/config `key: value` line (mirrors `^([\w-]+):\s*(.*)$`): the key must be
/// the whole run of `[A-Za-z0-9_-]` before the first `:`, with the trimmed remainder as the value.
fn match_config_line(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let key = line.get(..colon)?;
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let value = line.get(colon + 1..)?;
    Some((key, value.trim_start()))
}

// -------------------------------------------------------------------------------------------
// Package identity (identity.ts::parsePackageName / normalizePackageName / buildRuntimeName)
// -------------------------------------------------------------------------------------------

/// Port of `identity.ts::buildRuntimeName` — `{package}.{local}` when a non-empty package is set,
/// else the bare local name.
fn build_runtime_name(local: &str, package: Option<&str>) -> String {
    match package {
        Some(pkg) if !pkg.is_empty() => format!("{pkg}.{local}"),
        _ => local.to_string(),
    }
}

/// Port of `identity.ts::parsePackageName` for chain frontmatter/JSON `package`: `None`/empty ->
/// `Ok(None)`; a value that fails to normalize to a valid identifier -> `Err(<label> is invalid
/// after sanitization.)` (surfaced by the caller as a per-file [`ChainDiscoveryDiagnostic`]).
fn parse_chain_package_name(value: Option<&str>, label: &str) -> Result<Option<String>, String> {
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    normalize_valid_package_name(raw)
        .map(Some)
        .ok_or_else(|| format!("{label} is invalid after sanitization."))
}




// -------------------------------------------------------------------------------------------
// Acceptance validation (acceptance.ts::validateAcceptanceInput)
// -------------------------------------------------------------------------------------------

/// `validateAcceptanceInput` (`acceptance.ts:176-303` @v0.43.0), reached through the ONE
/// implementation this crate has — [`crate::exec::acceptance::model::validate_acceptance_input`].
///
/// Upstream has exactly one copy of this validator and every caller imports it: the chain
/// serializer this module ports does so explicitly
/// (`import { validateAcceptanceInput } from "../runs/shared/acceptance.ts"`,
/// `agents/chain-serializer.ts:5`, applied at `:178`/`:189`/`:197`), as do `agents/agents.ts:22`,
/// `agents/agent-management.ts:36` and `runs/background/async-execution.ts:32`. This file used to
/// carry a second, private ~260-line transcription of it plus its own `VALID_ACCEPTANCE_*` /
/// `ACCEPTANCE_*_KEYS` tables, which is how it came to be missing v0.43.0's duplicate-normalized-
/// criterion-id check and its `ACCEPTANCE_EVIDENCE_HELP`/`ACCEPTANCE_OBJECT_EXAMPLE` guidance
/// suffixes while the `model` copy was being updated.
///
/// The only adaptation is the `undefined` spelling: upstream passes a possibly-`undefined`
/// property straight in and returns no errors for it (`if (input === undefined) return errors`,
/// `acceptance.ts:178`), which the `model` signature spells as `Value::Null`.
fn validate_acceptance_input(input: Option<&Value>, path_label: &str) -> Vec<String> {
    crate::exec::acceptance::model::validate_acceptance_input(
        input.unwrap_or(&Value::Null),
        path_label,
    )
}

// -------------------------------------------------------------------------------------------
// Chain output-binding validation (chain-outputs.ts::validateChainOutputBindings)
// -------------------------------------------------------------------------------------------

/// Faithful port of `chain-outputs.ts::validateChainOutputBindings` (with the empty context pi uses
/// at parse time): named-output uniqueness + safe names, `{outputs.x}` reference resolution against
/// strictly-earlier steps, and dynamic-fanout shape validation. Errors surface as pi's
/// `ChainOutputValidationError` message text (wrapped `Invalid JSON chain '<path>': ...` by the
/// caller).
fn validate_chain_output_bindings(steps: &[Value]) -> Result<(), String> {
    let mut available: HashSet<String> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (step_index, step) in steps.iter().enumerate() {
        let display = step_index + 1;

        if has_dynamic_fanout_fields(step) {
            if !is_dynamic_parallel_step(step) {
                return Err(format!(
                    "Dynamic chain step {display} requires expand, a single parallel template object, and collect; dynamic expand/collect cannot be mixed with static parallel arrays."
                ));
            }
            validate_dynamic_step_shape(step, display, CHAIN_PARSE_MAX_ITEMS)?;
            let source_output = step
                .get("expand")
                .and_then(|expand| expand.get("from"))
                .and_then(|from| from.get("output"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !available.contains(source_output) {
                return Err(format!(
                    "Dynamic chain step {display} references unknown output '{source_output}'. Named outputs are only available after producing step/group completes."
                ));
            }
        }

        for name in output_names_for_step(step) {
            if !is_safe_output_name(&name) {
                return Err(format!(
                    "Invalid chain output name '{name}' at step {display}. Use /^[A-Za-z_][A-Za-z0-9_]*$/."
                ));
            }
            if seen.contains(&name) {
                return Err(format!(
                    "Duplicate chain output name '{name}'. Each as name must be unique."
                ));
            }
            seen.insert(name);
        }

        for template in task_templates_for_step(step) {
            for (raw_reference, name) in extract_output_refs(&template) {
                if !is_safe_output_name(&name) {
                    return Err(format!(
                        "Invalid chain output reference '{raw_reference}' at step {display}. Use {{outputs.name}} with /^[A-Za-z_][A-Za-z0-9_]*$/ names."
                    ));
                }
                if !available.contains(&name) {
                    return Err(format!(
                        "Unknown chain output reference '{raw_reference}' at step {display}. Named outputs are only available after producing step/group completes."
                    ));
                }
            }
        }

        for name in output_names_for_step(step) {
            available.insert(name);
        }
    }
    Ok(())
}

/// `settings.ts::isParallelStep`: has a `parallel` key whose value is an array.
fn is_parallel_step(step: &Value) -> bool {
    matches!(step.get("parallel"), Some(Value::Array(_)))
}

/// `settings.ts::isDynamicParallelStep`: has `expand` + `collect` + a non-array `parallel`.
fn is_dynamic_parallel_step(step: &Value) -> bool {
    step.get("expand").is_some()
        && step.get("collect").is_some()
        && step
            .get("parallel")
            .is_some_and(|parallel| !parallel.is_array())
}

/// `dynamic-fanout.ts::hasDynamicFanoutFields`: an object with an `expand` or `collect` key.
fn has_dynamic_fanout_fields(step: &Value) -> bool {
    step.is_object() && (step.get("expand").is_some() || step.get("collect").is_some())
}

/// `chain-outputs.ts::outputNamesForStep`: the named outputs a step registers (parallel tasks'
/// `as`, a dynamic step's `collect.as`, or a sequential step's `as`).
fn output_names_for_step(step: &Value) -> Vec<String> {
    if is_parallel_step(step) {
        return step
            .get("parallel")
            .and_then(Value::as_array)
            .map(|tasks| {
                tasks
                    .iter()
                    .filter_map(|task| task.get("as").and_then(Value::as_str))
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
    }
    if is_dynamic_parallel_step(step) {
        return step
            .get("collect")
            .and_then(|collect| collect.get("as"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(|name| vec![name.to_string()])
            .unwrap_or_default();
    }
    step.get("as")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(|name| vec![name.to_string()])
        .unwrap_or_default()
}

/// `chain-outputs.ts::taskTemplatesForStep`: the task-template strings a step's `{outputs.x}`
/// references are scanned in (each parallel task's `task`, a dynamic template's `task`+`label`, or
/// a sequential step's `task`), defaulting an absent task to `{previous}`.
fn task_templates_for_step(step: &Value) -> Vec<String> {
    if is_parallel_step(step) {
        return step
            .get("parallel")
            .and_then(Value::as_array)
            .map(|tasks| {
                tasks
                    .iter()
                    .map(|task| {
                        task.get("task")
                            .and_then(Value::as_str)
                            .unwrap_or("{previous}")
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    if is_dynamic_parallel_step(step) {
        let parallel = step.get("parallel");
        let task = parallel
            .and_then(|p| p.get("task"))
            .and_then(Value::as_str)
            .unwrap_or("{previous}")
            .to_string();
        let label = parallel
            .and_then(|p| p.get("label"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        return [task, label]
            .into_iter()
            .filter(|template| !template.is_empty())
            .collect();
    }
    vec![
        step.get("task")
            .and_then(Value::as_str)
            .unwrap_or("{previous}")
            .to_string(),
    ]
}

/// Extract every `{outputs.<name>}` reference from a template (mirrors `\{outputs\.([^}]*)\}`),
/// returning each `(raw_match, name)` pair.
fn extract_output_refs(template: &str) -> Vec<(String, String)> {
    const PREFIX: &str = "{outputs.";
    let mut refs = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find(PREFIX) {
        let after = rest.get(start + PREFIX.len()..).unwrap_or("");
        let Some(end) = after.find('}') else {
            break;
        };
        let name = after.get(..end).unwrap_or("");
        refs.push((format!("{{outputs.{name}}}"), name.to_string()));
        rest = after.get(end + 1..).unwrap_or("");
    }
    refs
}

/// `^[A-Za-z_][A-Za-z0-9_]*$` — a safe chain-output/item identifier.
fn is_safe_output_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Structural port of `dynamic-fanout.ts::validateDynamicStepShape` (key whitelists, `expand.from`,
/// safe names, JSON-Pointer syntax, `maxItems`, single-template `parallel`, `collect.as`), including
/// the deep per-item template-reference validation of `parallel.task`/`parallel.label`
/// (`assertNoUnresolvedItemReferences`, the `{item.path}` grammar in `dynamic-fanout.ts:208-213`) —
/// delegated to [`crate::spawn::dynamic_fanout::assert_no_unresolved_item_references`] (C16) so a
/// malformed item reference is rejected at chain-parse time, exactly as pi's
/// `validateChainOutputBindings` does.
pub(crate) fn validate_dynamic_step_shape(
    step: &Value,
    display: usize,
    config_max_items: u64,
) -> Result<(), String> {
    let prefix = format!("Dynamic chain step {display}");
    assert_only_keys(step, DYNAMIC_STEP_KEYS, &prefix)?;

    let expand = step.get("expand");
    let from = expand.and_then(|expand| expand.get("from"));
    let (Some(expand), Some(from)) = (expand, from) else {
        return Err(format!("{prefix} requires expand.from."));
    };
    assert_only_keys(expand, DYNAMIC_EXPAND_KEYS, &format!("{prefix} expand"))?;
    assert_only_keys(from, DYNAMIC_EXPAND_FROM_KEYS, &format!("{prefix} expand.from"))?;

    let output = from.get("output").and_then(Value::as_str).unwrap_or_default();
    if !is_safe_output_name(output) {
        return Err(format!(
            "{prefix} has invalid expand.from.output '{output}'."
        ));
    }
    let path = from.get("path").and_then(Value::as_str).unwrap_or_default();
    assert_json_pointer(path, &format!("{prefix} expand.from.path"))?;
    if let Some(key) = expand.get("key").and_then(Value::as_str) {
        assert_json_pointer(key, &format!("{prefix} expand.key"))?;
    }
    let item_name = expand.get("item").and_then(Value::as_str).unwrap_or("item");
    if !is_safe_output_name(item_name) {
        return Err(format!("{prefix} has invalid expand.item '{item_name}'."));
    }
    if let Some(max_items) = expand.get("maxItems")
        && !is_non_negative_integer(max_items)
    {
        return Err(format!("{prefix} expand.maxItems must be an integer >= 0."));
    }
    // `config.maxItems` is always the parse-time `MAX_SAFE_INTEGER`, so it is a valid non-negative
    // integer by construction and the "requires expand.maxItems or config.maxItems" branch (both
    // undefined) can never fire here — matching `chain-serializer.ts:204`.
    let _ = config_max_items;

    match step.get("parallel") {
        Some(parallel) if parallel.is_object() => {
            assert_only_keys(
                parallel,
                DYNAMIC_PARALLEL_KEYS,
                &format!("{prefix} parallel"),
            )?;
            if parallel.get("expand").is_some() {
                return Err(format!("{prefix} does not support nested dynamic fanout."));
            }
            if parallel
                .get("agent")
                .and_then(Value::as_str)
                .is_none_or(|agent| agent.is_empty())
            {
                return Err(format!("{prefix} parallel.agent is required."));
            }
            // C16 (`dynamic-fanout.ts:208-213`): reject a malformed / unknown item reference in the
            // `parallel.task`/`parallel.label` templates at parse time.
            for (label, field) in [("parallel.task", "task"), ("parallel.label", "label")] {
                if let Some(template) = parallel.get(field).and_then(Value::as_str)
                    && !template.is_empty()
                {
                    crate::spawn::dynamic_fanout::assert_no_unresolved_item_references(
                        template,
                        item_name,
                        &format!("{prefix} {label}"),
                    )?;
                }
            }
        }
        _ => {
            return Err(format!(
                "{prefix} requires a single parallel template object and cannot mix dynamic expand/collect with static parallel arrays."
            ));
        }
    }

    let collect_as = step
        .get("collect")
        .and_then(|collect| collect.get("as"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if collect_as.is_empty() || !is_safe_output_name(collect_as) {
        return Err(format!(
            "{prefix} requires collect.as with a safe output name."
        ));
    }
    if let Some(collect) = step.get("collect") {
        assert_only_keys(collect, DYNAMIC_COLLECT_KEYS, &format!("{prefix} collect"))?;
    }
    Ok(())
}

const DYNAMIC_STEP_KEYS: &[&str] = &[
    "expand",
    "parallel",
    "collect",
    "concurrency",
    "failFast",
    "phase",
    "label",
    "acceptance",
];
const DYNAMIC_EXPAND_KEYS: &[&str] = &["from", "item", "key", "maxItems", "onEmpty"];
const DYNAMIC_EXPAND_FROM_KEYS: &[&str] = &["output", "path"];
const DYNAMIC_PARALLEL_KEYS: &[&str] = &[
    "agent",
    "task",
    "phase",
    "label",
    "outputSchema",
    "cwd",
    "output",
    "outputMode",
    "reads",
    "progress",
    "skill",
    "model",
    "acceptance",
];
const DYNAMIC_COLLECT_KEYS: &[&str] = &["as", "outputSchema"];

/// `dynamic-fanout.ts::assertOnlyKeys`: the value must be a JSON object whose every key is in
/// `allowed`.
fn assert_only_keys(value: &Value, allowed: &[&str], label: &str) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Err(format!("{label} must be an object."));
    };
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{label} does not support field '{key}'."));
        }
    }
    Ok(())
}

/// `dynamic-fanout.ts::assertJsonPointer`: `""` is valid; otherwise it must start with `/` and
/// contain no invalid `~`-escape (a `~` not followed by `0` or `1`).
fn assert_json_pointer(pointer: &str, label: &str) -> Result<(), String> {
    if pointer.is_empty() {
        return Ok(());
    }
    let Some(rest) = pointer.strip_prefix('/') else {
        return Err(format!("{label} must be a JSON Pointer starting with '/'."));
    };
    for segment in rest.split('/') {
        if has_invalid_tilde_escape(segment) {
            return Err(format!("{label} contains invalid JSON Pointer escape."));
        }
    }
    Ok(())
}

/// A `~` not immediately followed by `0` or `1` (mirrors `/~(?![01])/`).
fn has_invalid_tilde_escape(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    for index in 0..bytes.len() {
        if bytes.get(index) == Some(&b'~')
            && !matches!(bytes.get(index + 1), Some(&b'0') | Some(&b'1'))
        {
            return true;
        }
    }
    false
}

/// `Number.isInteger(v) && v >= 0` for a JSON value (accepts a whole-number float like `4.0`, which
/// `Number.isInteger` treats as an integer).
fn is_non_negative_integer(value: &Value) -> bool {
    if value.as_u64().is_some() {
        return true;
    }
    value
        .as_f64()
        .is_some_and(|f| f >= 0.0 && f.fract() == 0.0)
}

// -------------------------------------------------------------------------------------------
// Authoring -> runtime bridge (ChainStepConfig -> RunnerStep)
// -------------------------------------------------------------------------------------------

/// Convert one parsed [`ChainStepConfig`] authoring shape into the runtime dispatch form
/// [`RunnerStep`] the chain-graph walker executes. The step's shape selects the variant exactly as
/// pi's runtime step guards do: an array `parallel` -> [`RunnerStep::ParallelGroup`]; an
/// `expand`+`collect` with an object `parallel` -> [`RunnerStep::DynamicGroup`]; otherwise
/// [`RunnerStep::SingleStep`].
///
/// This is a STRUCTURAL bridge: it carries the real agent NAME (never a placeholder persona — name
/// resolution to a full `AgentConfig` remains the executor's job), the task, and the fields
/// `SingleStepSpec` has a home for (`output` = pi's `as` named-output key, `output_mode`, `reads`),
/// exactly mirroring `registration::slash_commands::step_token_to_spec`'s established mapping. Like
/// that converter, it defers to a later phase (T0.1 plan-time enrichment): the per-step `model`
/// string is not resolved to a `ModelId` here (`model: None` -> the persona's own model), a
/// path-form `outputSchema` is not loaded into `structured_output_schema`, and a static
/// `parallel`/`dynamic` group's `concurrency` falls back to `default_concurrency` when the step
/// omits it. The step's `acceptance` IS carried through verbatim (SUBA-N04) — lowering it to a
/// runtime contract is `run_single`'s job, at dispatch, exactly as upstream does it.
pub fn chain_step_to_runner_step(step: &ChainStepConfig, default_concurrency: u32) -> RunnerStep {
    if let Some(Value::Array(items)) = &step.parallel {
        let steps: Vec<SingleStepSpec> = items.iter().filter_map(value_to_single_step_spec).collect();
        return RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps,
            concurrency: chain_concurrency(step, default_concurrency),
            fail_fast: step.fail_fast.unwrap_or(false),
            worktree: step.worktree.unwrap_or(false),
        });
    }

    if let (Some(expand), Some(collect), Some(parallel)) =
        (&step.expand, &step.collect, &step.parallel)
        && parallel.is_object()
    {
        let template = value_to_single_step_spec(parallel).unwrap_or_else(empty_single_step_spec);
        return RunnerStep::DynamicGroup(DynamicGroupSpec {
            expand: dynamic_expand_pointer(expand),
            template: Box::new(template),
            collect: collect
                .get("as")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            concurrency: chain_concurrency(step, default_concurrency),
            // `failFast` is a legal dynamic-step key at the ported baseline
            // (`dynamic-fanout.ts:44` `DYNAMIC_STEP_KEYS`, mirrored by this file's own
            // `DYNAMIC_STEP_KEYS`) and upstream forwards it verbatim when it lowers the dynamic
            // step to a `ParallelStep` (`chain-execution.ts:1061-1067`: `failFast: step.failFast`),
            // where `runParallelChainTasks` applies pi's `?? false` default
            // (`chain-execution.ts:283`). Reading it here — exactly as the static-`parallel` arm
            // above does — is what keeps the validator's acceptance of the key honest.
            fail_fast: step.fail_fast.unwrap_or(false),
            // C16: carry pi's `expand.{item,key,maxItems,onEmpty}` and `collect.outputSchema`
            // through to the runtime `DynamicGroupSpec` so the walker can substitute each item's
            // task, cap/dedup the fan-out, and validate the collect record shape.
            item: expand
                .get("item")
                .and_then(Value::as_str)
                .map(str::to_string),
            key: expand.get("key").and_then(Value::as_str).map(str::to_string),
            max_items: expand
                .get("maxItems")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok()),
            on_empty: match expand.get("onEmpty").and_then(Value::as_str) {
                Some("fail") => OnEmpty::Fail,
                _ => OnEmpty::Skip,
            },
            collect_schema: collect.get("outputSchema").cloned(),
            // SUBA-C14: the GROUP-level `acceptance` gate, carried RAW exactly as the single-step
            // arm below carries its own (SUBA-N04). `acceptance` is a legal dynamic-step key
            // upstream (`dynamic-fanout.ts:45` `DYNAMIC_STEP_KEYS`, mirrored by this file's own
            // `DYNAMIC_STEP_KEYS`) and `chain-execution.ts:1034-1055` evaluates it against the
            // aggregate child report once the group settles, failing the whole chain on rejection.
            // Dropping it here — as this arm did before — left a validator-accepted gate inert.
            acceptance: step
                .acceptance
                .clone()
                .filter(|value| !value.is_null()),
        });
    }

    RunnerStep::SingleStep(chain_step_to_single_step_spec(step))
}

/// Build a [`DynamicGroupSpec::expand`] pointer (`outputs.<name>[<json-pointer>]`) from a chain
/// step's `expand.from.{output,path}` — the shape [`crate::spawn::chain_graph::OutputRegistry::
/// resolve_pointer`] consumes.
fn dynamic_expand_pointer(expand: &Value) -> String {
    let from = expand.get("from");
    let output = from
        .and_then(|from| from.get("output"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = from
        .and_then(|from| from.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("outputs.{output}{path}")
}

fn chain_concurrency(step: &ChainStepConfig, default_concurrency: u32) -> u32 {
    step.concurrency
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(default_concurrency)
}

/// Convert one raw parallel-task-item [`Value`] into a [`SingleStepSpec`] by first deserializing it
/// into a [`ChainStepConfig`] (skipping any element that is not a well-formed step object).
fn value_to_single_step_spec(value: &Value) -> Option<SingleStepSpec> {
    let config: ChainStepConfig = serde_json::from_value(value.clone()).ok()?;
    Some(chain_step_to_single_step_spec(&config))
}

fn empty_single_step_spec() -> SingleStepSpec {
    chain_step_to_single_step_spec(&ChainStepConfig::default())
}

/// Map a sequential [`ChainStepConfig`] onto a [`SingleStepSpec`] (see
/// [`chain_step_to_runner_step`] for the deferral rationale).
fn chain_step_to_single_step_spec(step: &ChainStepConfig) -> SingleStepSpec {
    SingleStepSpec {
        skills: None,
        session_dir: None,
        agent: step.agent.clone().unwrap_or_default(),
        task: step.task.clone().unwrap_or_default(),
        cwd: step
            .extra
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from),
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: step.as_.clone(),
        // pi's `output` binding is the output FILE path (`ChainOutputBinding::Name`); the
        // `Toggle(false)` "no output" sentinel and an empty string both map to `None` (no file).
        // Distinct from `output` above, which carries pi's `as` registry KEY.
        output_path: match &step.output {
            Some(crate::discovery::types::ChainOutputBinding::Name(path)) if !path.is_empty() => {
                Some(path.clone())
            }
            _ => None,
        },
        output_mode: step.output_mode.as_deref().and_then(parse_output_mode),
        reads: match &step.reads {
            Some(ChainListBinding::List(paths)) => {
                Some(paths.iter().map(PathBuf::from).collect())
            }
            Some(ChainListBinding::Toggle(_)) | None => None,
        },
        // SUBA-N04: the RAW acceptance value, carried whole. This used to be
        // `.and_then(Value::as_str)`, which kept only the level-string form and silently discarded
        // the `false` shorthand AND every `{ level, verify: [{ command }], … }` object — i.e. the
        // only forms that can declare a `verify[]` command at all. `run_single` lowers whatever is
        // here through `exec::acceptance::lower_acceptance_input` (pi `chain-execution.ts:1335`
        // passes `seqStep.acceptance` into `runSync` unmodified for exactly this reason). `null` is
        // normalized to `None` so an explicit JSON `null` reads as pi's `undefined`.
        acceptance: step
            .acceptance
            .as_ref()
            .filter(|value| !value.is_null())
            .cloned(),
        context: None,
        agent_scope: None,
    }
}

fn parse_output_mode(value: &str) -> Option<OutputMode> {
    match value {
        "inline" => Some(OutputMode::Inline),
        "file-only" => Some(OutputMode::FileOnly),
        "file-and-inline" => Some(OutputMode::FileAndInline),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write fixture chain file");
        path
    }

    /// A real pi-format `.chain.json`: root object with `name`/`description`/`chain` array. Each
    /// step is a minimal sequential step with a distinct agent/task.
    fn sample_json(steps: usize) -> String {
        let steps_json: Vec<String> = (0..steps)
            .map(|i| format!("{{\"agent\":\"agent-{i}\",\"task\":\"task-{i}\"}}"))
            .collect();
        format!(
            "{{\"name\":\"release\",\"description\":\"release chain\",\"chain\":[{}]}}",
            steps_json.join(",")
        )
    }

    /// A real pi-format `.chain.md`: frontmatter `name`/`description`, then one `## agent-i` section
    /// per step, each with a distinct task after the blank line.
    fn sample_md(steps: usize) -> String {
        let mut body = String::new();
        for i in 0..steps {
            if i > 0 {
                body.push('\n');
            }
            body.push_str(&format!("## agent-{i}\n\ntask-{i}\n"));
        }
        format!("---\nname: release\ndescription: release chain (md)\n---\n\n{body}")
    }

    // ---- The task's three required fixtures ----

    #[test]
    fn two_step_chain_md_parses_to_two_steps_with_correct_agent_task_and_config() {
        let content = "---\nname: review-chain\ndescription: Review chain\n---\n\n## reviewer\noutput: report.md\noutputMode: file-only\nas: reviewNotes\n\nReview the diff\n\n## fixer\nmodel: fast-model\nreads: src/a.rs, src/b.rs\n\nApply the fixes from {outputs.reviewNotes}\n";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "review-chain.chain.md", content);

        let def = parse_chain_md(&path, AgentSource::Project).expect("parse md chain");
        assert_eq!(def.name, "review-chain");
        assert_eq!(def.local_name, "review-chain");
        assert_eq!(def.package_name, None);
        assert_eq!(def.description, "Review chain");
        assert_eq!(def.steps.len(), 2);

        let first = &def.steps[0];
        assert_eq!(first.agent.as_deref(), Some("reviewer"));
        assert_eq!(first.task.as_deref(), Some("Review the diff"));
        assert_eq!(
            first.output,
            Some(ChainOutputBinding::Name("report.md".to_string()))
        );
        assert_eq!(first.output_mode.as_deref(), Some("file-only"));
        assert_eq!(first.as_.as_deref(), Some("reviewNotes"));

        let second = &def.steps[1];
        assert_eq!(second.agent.as_deref(), Some("fixer"));
        assert_eq!(
            second.task.as_deref(),
            Some("Apply the fixes from {outputs.reviewNotes}")
        );
        assert_eq!(second.model.as_deref(), Some("fast-model"));
        assert_eq!(
            second.reads,
            Some(ChainListBinding::List(vec![
                "src/a.rs".to_string(),
                "src/b.rs".to_string()
            ]))
        );
    }

    #[test]
    fn chain_json_with_chain_array_parses_correctly() {
        let content = "{\"name\":\"dynamic-review\",\"description\":\"Review dynamic targets\",\"chain\":[{\"agent\":\"scout\",\"task\":\"Return targets\",\"as\":\"targets\",\"outputSchema\":{\"type\":\"object\"}},{\"expand\":{\"from\":{\"output\":\"targets\",\"path\":\"/items\"},\"item\":\"target\",\"key\":\"/path\",\"maxItems\":4},\"parallel\":{\"agent\":\"reviewer\",\"task\":\"Review {target.path}\",\"outputSchema\":{\"type\":\"object\"}},\"collect\":{\"as\":\"reviews\"}}]}";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "dynamic-review.chain.json", content);

        let def = parse_chain_json(&path, AgentSource::Project).expect("parse json chain");
        assert_eq!(def.name, "dynamic-review");
        assert_eq!(def.steps.len(), 2);
        assert_eq!(def.steps[0].agent.as_deref(), Some("scout"));
        assert_eq!(def.steps[0].as_.as_deref(), Some("targets"));
        assert_eq!(
            def.steps[0].output_schema,
            Some(serde_json::json!({ "type": "object" }))
        );
        // The dynamic step retained its raw collect/expand/parallel shapes.
        assert_eq!(
            def.steps[1].collect,
            Some(serde_json::json!({ "as": "reviews" }))
        );
        assert!(def.steps[1].expand.is_some());
        assert!(def.steps[1].parallel.is_some());
    }

    #[test]
    fn packaged_chain_gets_its_qualified_runtime_name() {
        let json_content = "{\"name\":\"release\",\"package\":\"code-analysis\",\"description\":\"Packaged release chain\",\"chain\":[{\"agent\":\"scout\",\"task\":\"go\"}]}";
        let tmp = tempfile::tempdir().expect("tempdir");
        let json_path = write(tmp.path(), "release.chain.json", json_content);
        let json_def = parse_chain_json(&json_path, AgentSource::User).expect("parse json chain");
        assert_eq!(json_def.name, "code-analysis.release");
        assert_eq!(json_def.local_name, "release");
        assert_eq!(json_def.package_name.as_deref(), Some("code-analysis"));

        let md_content = "---\nname: release\npackage: Code Analysis\ndescription: Packaged release chain (md)\n---\n\n## scout\n\ngo\n";
        let md_path = write(tmp.path(), "release-md.chain.md", md_content);
        let md_def = parse_chain_md(&md_path, AgentSource::User).expect("parse md chain");
        // `Code Analysis` normalizes to `code-analysis` (lowercase, whitespace -> hyphen).
        assert_eq!(md_def.name, "code-analysis.release");
        assert_eq!(md_def.package_name.as_deref(), Some("code-analysis"));
    }

    // ---- Pinned pi chain-serializer.test.ts behaviors ----

    #[test]
    fn inline_output_schema_in_md_is_rejected() {
        let content = "---\nname: review-chain\ndescription: Review chain\n---\n\n## reviewer\noutputSchema: {\"type\":\"object\"}\n\nReview the diff\n";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "review-chain.chain.md", content);
        let err = parse_chain_md(&path, AgentSource::Project).expect_err("inline schema rejected");
        assert!(
            err.contains("Inline outputSchema values are not supported"),
            "{err}"
        );
    }

    #[test]
    fn md_chain_missing_name_or_description_errors() {
        let content = "---\nname: only-name\n---\n\n## reviewer\n\ntask\n";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "broken.chain.md", content);
        let err = parse_chain_md(&path, AgentSource::Project).expect_err("missing description");
        assert!(
            err.contains("Chain frontmatter must include name and description"),
            "{err}"
        );
    }

    #[test]
    fn json_chain_missing_chain_array_errors() {
        let content = "{\"name\":\"n\",\"description\":\"d\"}";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "no-chain.chain.json", content);
        let err = parse_chain_json(&path, AgentSource::User).expect_err("missing chain array");
        assert!(err.contains("must include array chain"), "{err}");
    }

    #[test]
    fn json_chain_non_object_step_is_rejected() {
        let content = "{\"name\":\"bad\",\"description\":\"Bad\",\"chain\":[1]}";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "bad.chain.json", content);
        let err = parse_chain_json(&path, AgentSource::User).expect_err("non-object step");
        assert!(err.contains("step 1 must be an object"), "{err}");
    }

    #[test]
    fn json_chain_bad_acceptance_reason_required() {
        let content = "{\"name\":\"bad-acceptance\",\"description\":\"Bad acceptance\",\"chain\":[{\"agent\":\"worker\",\"acceptance\":{\"level\":\"none\"}}]}";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "bad-acceptance.chain.json", content);
        let err = parse_chain_json(&path, AgentSource::User).expect_err("acceptance reason required");
        assert!(err.contains("step 1 acceptance.reason is required"), "{err}");
    }

    /// G78 — the chain-JSON validator is a SECOND copy of `validateAcceptanceInput`, so it has to
    /// refuse `reviewed` (and reasonless `none` / command-less `verified`) with the same messages
    /// the tool-param path uses. A chain file is exactly where a stale `"reviewed"` would otherwise
    /// sit unnoticed until dispatch.
    #[test]
    fn json_chain_rejects_levels_that_are_no_longer_requestable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cases = [
            (
                "{\"agent\":\"worker\",\"acceptance\":\"reviewed\"}",
                "is an achieved status, not a requestable acceptance level",
            ),
            (
                "{\"agent\":\"worker\",\"acceptance\":{\"level\":\"reviewed\"}}",
                "step 1 acceptance.level is an achieved status",
            ),
            (
                "{\"agent\":\"worker\",\"acceptance\":\"none\"}",
                "requires a reason",
            ),
            (
                "{\"agent\":\"worker\",\"acceptance\":\"verified\"}",
                "requires object form with at least one runtime verify command",
            ),
            (
                "{\"agent\":\"worker\",\"acceptance\":{\"level\":\"verified\"}}",
                "must contain at least one runtime command when level is verified",
            ),
        ];
        for (step, expected) in cases {
            let content = format!(
                "{{\"name\":\"c\",\"description\":\"d\",\"chain\":[{step}]}}"
            );
            let path = write(tmp.path(), "levels.chain.json", &content);
            let err = parse_chain_json(&path, AgentSource::User)
                .expect_err("the level must be refused at parse time");
            assert!(err.contains(expected), "for {step}: {err}");
        }
        // The still-requestable bare levels keep parsing.
        for level in ["auto", "attested", "checked"] {
            let content = format!(
                "{{\"name\":\"c\",\"description\":\"d\",\"chain\":[{{\"agent\":\"worker\",\"acceptance\":\"{level}\"}}]}}"
            );
            let path = write(tmp.path(), "ok.chain.json", &content);
            assert!(
                parse_chain_json(&path, AgentSource::User).is_ok(),
                "`{level}` must remain requestable in a chain file"
            );
        }
    }

    /// The two NESTED acceptance-validation arms of `parse_chain_json` — a static `parallel[]`
    /// task's own `acceptance` and a dynamic step's single `parallel` TEMPLATE object's — which the
    /// step-level test above never reaches.
    ///
    /// pi validates all three levels in one pass (`validateExecutionAcceptance`,
    /// `runs/shared/acceptance.ts:305-328` @v0.43.0: the top-level `acceptance`, every `tasks[i]`
    /// and every `chain[i]`), so a chain FILE has to be just as thorough — a saved chain is
    /// authored once and dispatched forever, and an invalid policy buried in a fan-out task or a
    /// dynamic template would otherwise surface only at spawn time, mid-run.
    ///
    /// Both arms are asserted on their own PATH LABEL, because that label is the only thing that
    /// tells the author which of a dozen tasks is at fault.
    #[test]
    fn json_chain_validates_acceptance_on_parallel_tasks_and_on_the_dynamic_template() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // (1) The static-`parallel[]` arm. The SECOND task carries the bad policy, so the index in
        // the label has to be right, not merely present.
        let content = "{\"name\":\"c\",\"description\":\"d\",\"chain\":[{\"parallel\":[\
             {\"agent\":\"a\",\"task\":\"ta\"},\
             {\"agent\":\"b\",\"task\":\"tb\",\"acceptance\":{\"level\":\"none\"}}]}]}";
        let path = write(tmp.path(), "parallel-task.chain.json", content);
        let err = parse_chain_json(&path, AgentSource::User)
            .expect_err("a fan-out task's own acceptance policy is validated too");
        assert!(
            err.contains("step 1 parallel task 2 acceptance.reason is required when level is none"),
            "{err}"
        );

        // (2) The dynamic-template arm: `parallel` is a single object, not an array.
        let content = "{\"name\":\"c\",\"description\":\"d\",\"chain\":[\
             {\"agent\":\"scout\",\"task\":\"Return targets\",\"as\":\"targets\",\"outputSchema\":{\"type\":\"object\"}},\
             {\"expand\":{\"from\":{\"output\":\"targets\",\"path\":\"/items\"},\"item\":\"target\",\"maxItems\":4},\
              \"parallel\":{\"agent\":\"reviewer\",\"task\":\"Review {target.path}\",\"acceptance\":\"reviewed\"},\
              \"collect\":{\"as\":\"reviews\"}}]}";
        let path = write(tmp.path(), "dynamic-template.chain.json", content);
        let err = parse_chain_json(&path, AgentSource::User)
            .expect_err("a dynamic template's acceptance policy is validated too");
        assert!(
            err.contains("step 2 dynamic template acceptance")
                && err.contains("is an achieved status, not a requestable acceptance level"),
            "{err}"
        );

        // (3) The control: the SAME two shapes with VALID policies parse, so neither arm is simply
        // rejecting every nested `acceptance` it sees.
        let content = "{\"name\":\"c\",\"description\":\"d\",\"chain\":[{\"parallel\":[\
             {\"agent\":\"a\",\"task\":\"ta\",\"acceptance\":\"attested\"},\
             {\"agent\":\"b\",\"task\":\"tb\",\"acceptance\":{\"level\":\"none\",\"reason\":\"trivial\"}}]}]}";
        let path = write(tmp.path(), "parallel-ok.chain.json", content);
        parse_chain_json(&path, AgentSource::User).expect("valid fan-out policies parse");

        let content = "{\"name\":\"c\",\"description\":\"d\",\"chain\":[\
             {\"agent\":\"scout\",\"task\":\"Return targets\",\"as\":\"targets\",\"outputSchema\":{\"type\":\"object\"}},\
             {\"expand\":{\"from\":{\"output\":\"targets\",\"path\":\"/items\"},\"item\":\"target\",\"maxItems\":4},\
              \"parallel\":{\"agent\":\"reviewer\",\"task\":\"Review {target.path}\",\"acceptance\":\"checked\"},\
              \"collect\":{\"as\":\"reviews\"}}]}";
        let path = write(tmp.path(), "dynamic-ok.chain.json", content);
        parse_chain_json(&path, AgentSource::User).expect("a valid dynamic-template policy parses");
    }

    #[test]
    fn json_chain_valid_acceptance_parses() {
        let content = "{\"name\":\"accepted-chain\",\"description\":\"Chain with acceptance gates\",\"chain\":[{\"agent\":\"worker\",\"task\":\"Fix bug\",\"acceptance\":{\"level\":\"checked\",\"evidence\":[\"changed-files\",\"commands-run\"]}},{\"parallel\":[{\"agent\":\"reviewer\",\"task\":\"Review\",\"acceptance\":\"attested\"}]}]}";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "accepted-chain.chain.json", content);
        let def = parse_chain_json(&path, AgentSource::Project).expect("valid acceptance parses");
        assert_eq!(def.steps.len(), 2);
        assert_eq!(
            def.steps[0].acceptance,
            Some(serde_json::json!({ "level": "checked", "evidence": ["changed-files", "commands-run"] }))
        );
    }

    #[test]
    fn invalid_dynamic_chain_mixing_static_parallel_arrays_is_rejected() {
        let content = "{\"name\":\"bad-dynamic-review\",\"description\":\"Bad dynamic targets\",\"chain\":[{\"agent\":\"scout\",\"task\":\"Return targets\",\"as\":\"targets\",\"outputSchema\":{\"type\":\"object\"}},{\"expand\":{\"from\":{\"output\":\"targets\",\"path\":\"/items\"},\"maxItems\":4},\"parallel\":[{\"agent\":\"reviewer\",\"task\":\"Review\"}],\"collect\":{\"as\":\"reviews\"}}]}";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "bad-dynamic-review.chain.json", content);
        let err = parse_chain_json(&path, AgentSource::Project).expect_err("static parallel arrays");
        assert!(err.contains("static parallel arrays"), "{err}");
    }

    #[test]
    fn duplicate_chain_output_names_are_rejected() {
        let content = "{\"name\":\"dupe\",\"description\":\"Dupe outputs\",\"chain\":[{\"agent\":\"a\",\"task\":\"t\",\"as\":\"x\"},{\"agent\":\"b\",\"task\":\"t\",\"as\":\"x\"}]}";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "dupe.chain.json", content);
        let err = parse_chain_json(&path, AgentSource::User).expect_err("duplicate output name");
        assert!(err.contains("Duplicate chain output name 'x'"), "{err}");
    }

    #[test]
    fn unknown_output_reference_is_rejected() {
        let content = "{\"name\":\"ref\",\"description\":\"Bad ref\",\"chain\":[{\"agent\":\"a\",\"task\":\"use {outputs.missing}\"}]}";
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "ref.chain.json", content);
        let err = parse_chain_json(&path, AgentSource::User).expect_err("unknown output ref");
        assert!(err.contains("Unknown chain output reference"), "{err}");
    }

    // ---- Directory scan / R-SA-015 precedence (updated to pi-format fixtures) ----

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
        let mut by_name: HashMap<String, ChainCandidate> = HashMap::new();
        let json_def = |steps: usize| ChainDefinition {
            name: "release".to_string(),
            local_name: "release".to_string(),
            package_name: None,
            description: "json".to_string(),
            source: AgentSource::User,
            file_path: PathBuf::from("/scope/release.chain.json"),
            steps: vec![ChainStepConfig::default(); steps],
            extra_fields: BTreeMap::new(),
        };
        let md_def = ChainDefinition {
            name: "release".to_string(),
            local_name: "release".to_string(),
            package_name: None,
            description: "md".to_string(),
            source: AgentSource::User,
            file_path: PathBuf::from("/scope/release.chain.md"),
            steps: vec![ChainStepConfig::default()],
            extra_fields: BTreeMap::new(),
        };

        // Insert Json first, then Md: Md must not win.
        insert_with_format_precedence(
            &mut by_name,
            "release".to_string(),
            json_def(2),
            ChainFileFormat::Json,
        );
        insert_with_format_precedence(
            &mut by_name,
            "release".to_string(),
            md_def,
            ChainFileFormat::Md,
        );
        assert_eq!(
            by_name.get("release").expect("winner present").definition.file_path,
            PathBuf::from("/scope/release.chain.json")
        );

        // Insert Md first, then Json: Json must win.
        let mut by_name2: HashMap<String, ChainCandidate> = HashMap::new();
        insert_with_format_precedence(
            &mut by_name2,
            "release".to_string(),
            ChainDefinition {
                name: "release".to_string(),
                local_name: "release".to_string(),
                package_name: None,
                description: "md".to_string(),
                source: AgentSource::User,
                file_path: PathBuf::from("/scope/release.chain.md"),
                steps: vec![ChainStepConfig::default()],
                extra_fields: BTreeMap::new(),
            },
            ChainFileFormat::Md,
        );
        insert_with_format_precedence(
            &mut by_name2,
            "release".to_string(),
            json_def(2),
            ChainFileFormat::Json,
        );
        assert_eq!(
            by_name2.get("release").expect("winner present").definition.file_path,
            PathBuf::from("/scope/release.chain.json")
        );
    }

    #[test]
    fn both_formats_parse_into_the_same_chain_definition_shape() {
        let json_tmp = tempfile::tempdir().expect("tempdir");
        write(json_tmp.path(), "release.chain.json", &sample_json(2));
        let json_result = scan_chain_dir(json_tmp.path(), AgentSource::User);
        assert!(json_result.diagnostics.is_empty(), "{:?}", json_result.diagnostics);
        assert_eq!(json_result.chains.len(), 1);
        let from_json = &json_result.chains[0];

        let md_tmp = tempfile::tempdir().expect("tempdir");
        write(md_tmp.path(), "release.chain.md", &sample_md(2));
        let md_result = scan_chain_dir(md_tmp.path(), AgentSource::User);
        assert!(md_result.diagnostics.is_empty(), "{:?}", md_result.diagnostics);
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
        assert!(result.chains.iter().any(|c| c.source == AgentSource::Project));
    }

    #[test]
    fn malformed_json_chain_file_produces_diagnostic_not_abort() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "broken.chain.json", "{ not valid json ");
        write(tmp.path(), "release.chain.json", &sample_json(1));

        let result = scan_chain_dir(tmp.path(), AgentSource::Project);
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].file_path.ends_with("broken.chain.json"));
        assert_eq!(result.chains.len(), 1);
        assert_eq!(result.chains[0].name, "release");
    }

    #[test]
    fn malformed_md_chain_file_missing_frontmatter_produces_diagnostic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "broken.chain.md", "no frontmatter here at all\n");

        let result = scan_chain_dir(tmp.path(), AgentSource::User);
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.chains.is_empty());
    }

    #[test]
    fn nested_subdirectories_are_scanned_recursively() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir nested");
        write(
            &nested,
            "deep.chain.json",
            "{\"name\":\"deep\",\"description\":\"deep chain\",\"chain\":[{\"agent\":\"a\",\"task\":\"t\"}]}",
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
    fn empty_chain_array_yields_zero_steps_not_malformed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "empty.chain.json",
            "{\"name\":\"empty\",\"description\":\"d\",\"chain\":[]}",
        );
        let result = scan_chain_dir(tmp.path(), AgentSource::User);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.chains.len(), 1);
        assert!(result.chains[0].steps.is_empty());
    }

    #[test]
    fn json_chain_missing_name_produces_diagnostic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "unnamed.chain.json", "{\"chain\":[]}");
        let result = scan_chain_dir(tmp.path(), AgentSource::User);
        assert!(result.chains.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].message.contains("must include string name"));
    }

    // ---- Authoring -> runtime bridge ----

    #[test]
    fn chain_step_to_runner_step_maps_sequential_agent_and_task() {
        let step = ChainStepConfig {
            agent: Some("reviewer".to_string()),
            task: Some("review it".to_string()),
            as_: Some("notes".to_string()),
            ..ChainStepConfig::default()
        };
        match chain_step_to_runner_step(&step, 4) {
            RunnerStep::SingleStep(spec) => {
                assert_eq!(spec.agent, "reviewer");
                assert_eq!(spec.task, "review it");
                assert_eq!(spec.output.as_deref(), Some("notes"));
                assert!(spec.model.is_none());
            }
            other => panic!("expected SingleStep, got {other:?}"),
        }
    }

    #[test]
    fn chain_step_to_runner_step_maps_static_parallel_group() {
        let step = ChainStepConfig {
            parallel: Some(serde_json::json!([
                { "agent": "a", "task": "ta" },
                { "agent": "b", "task": "tb" }
            ])),
            concurrency: Some(2),
            ..ChainStepConfig::default()
        };
        match chain_step_to_runner_step(&step, 8) {
            RunnerStep::ParallelGroup(group) => {
                assert_eq!(group.steps.len(), 2);
                assert_eq!(group.concurrency, 2);
                assert_eq!(group.steps[0].agent, "a");
                assert_eq!(group.steps[1].agent, "b");
            }
            other => panic!("expected ParallelGroup, got {other:?}"),
        }
    }

    #[test]
    fn chain_step_to_runner_step_maps_dynamic_group_expand_pointer() {
        let step = ChainStepConfig {
            expand: Some(serde_json::json!({ "from": { "output": "targets", "path": "/items" } })),
            parallel: Some(serde_json::json!({ "agent": "reviewer", "task": "review" })),
            collect: Some(serde_json::json!({ "as": "reviews" })),
            concurrency: Some(3),
            ..ChainStepConfig::default()
        };
        match chain_step_to_runner_step(&step, 8) {
            RunnerStep::DynamicGroup(group) => {
                assert_eq!(group.expand, "outputs.targets/items");
                assert_eq!(group.collect, "reviews");
                assert_eq!(group.concurrency, 3);
                assert_eq!(group.template.agent, "reviewer");
            }
            other => panic!("expected DynamicGroup, got {other:?}"),
        }
    }

    /// A dynamic-fanout step's `failFast` must survive lowering. `DYNAMIC_STEP_KEYS` accepts the
    /// key (mirroring `dynamic-fanout.ts:44` @v0.34.0) and `ChainStepConfig::fail_fast` parses it,
    /// but this bridge previously read it ONLY on the static-`parallel` arm and dropped it on the
    /// dynamic arm — so an author's `failFast: true` was validated as legal and then silently
    /// ignored. Upstream forwards it verbatim when it lowers the dynamic step to a `ParallelStep`
    /// (`chain-execution.ts:1061-1067` @v0.43.0: `failFast: step.failFast`), and applies pi's `??
    /// false` default only at dispatch (`chain-execution.ts:283`).
    #[test]
    fn chain_step_to_runner_step_carries_fail_fast_onto_a_dynamic_group() {
        let dynamic = |fail_fast: Option<bool>| ChainStepConfig {
            expand: Some(serde_json::json!({ "from": { "output": "targets", "path": "/items" } })),
            parallel: Some(serde_json::json!({ "agent": "reviewer", "task": "review" })),
            collect: Some(serde_json::json!({ "as": "reviews" })),
            fail_fast,
            ..ChainStepConfig::default()
        };

        match chain_step_to_runner_step(&dynamic(Some(true)), 8) {
            RunnerStep::DynamicGroup(group) => assert!(
                group.fail_fast,
                "`failFast: true` on a dynamic step must reach DynamicGroupSpec::fail_fast"
            ),
            other => panic!("expected DynamicGroup, got {other:?}"),
        }

        // Absent and explicit-`false` both lower to pi's `?? false`.
        for omitted in [None, Some(false)] {
            match chain_step_to_runner_step(&dynamic(omitted), 8) {
                RunnerStep::DynamicGroup(group) => assert!(
                    !group.fail_fast,
                    "a dynamic step without `failFast: true` must default to false ({omitted:?})"
                ),
                other => panic!("expected DynamicGroup, got {other:?}"),
            }
        }
    }

    /// SUBA-N04: a saved chain file's per-step `acceptance` reaches the runtime step spec WHOLE, in
    /// every form — including the `{ level, verify: [{ command }] }` object, which is the only form
    /// that can declare a `verify[]` command and which this bridge previously discarded outright
    /// (`.and_then(Value::as_str)` kept the bare level string and nothing else). Lowering it to a
    /// contract stays `run_single`'s job, exactly as upstream hands `seqStep.acceptance` to `runSync`
    /// unmodified (pi `chain-execution.ts:1401` @v0.43.0).
    #[test]
    fn chain_step_to_runner_step_carries_every_acceptance_form_onto_the_step_spec() {
        let policy = serde_json::json!({
            "level": "verified",
            "verify": [{ "id": "unit", "command": "cargo test" }]
        });
        let step = ChainStepConfig {
            agent: Some("builder".to_string()),
            task: Some("fix it".to_string()),
            acceptance: Some(policy.clone()),
            ..ChainStepConfig::default()
        };
        match chain_step_to_runner_step(&step, 4) {
            RunnerStep::SingleStep(spec) => assert_eq!(spec.acceptance, Some(policy)),
            other => panic!("expected SingleStep, got {other:?}"),
        }

        // A static parallel task's own policy survives the per-item `Value` -> spec hop too.
        let group = ChainStepConfig {
            parallel: Some(serde_json::json!([
                { "agent": "a", "task": "ta", "acceptance": false },
                { "agent": "b", "task": "tb", "acceptance": "checked" }
            ])),
            ..ChainStepConfig::default()
        };
        match chain_step_to_runner_step(&group, 8) {
            RunnerStep::ParallelGroup(group) => {
                assert_eq!(group.steps[0].acceptance, Some(serde_json::json!(false)));
                assert_eq!(group.steps[1].acceptance, Some(serde_json::json!("checked")));
            }
            other => panic!("expected ParallelGroup, got {other:?}"),
        }

        // No policy at all stays `None` — pi's `undefined`, which defers to the heuristic default.
        let bare = ChainStepConfig {
            agent: Some("c".to_string()),
            ..ChainStepConfig::default()
        };
        match chain_step_to_runner_step(&bare, 4) {
            RunnerStep::SingleStep(spec) => assert_eq!(spec.acceptance, None),
            other => panic!("expected SingleStep, got {other:?}"),
        }
    }
}
