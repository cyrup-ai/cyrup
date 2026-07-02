//! Agent/chain management CRUD: create/update/delete/rename (func-SA §5.1 R-SA-013/014/019;
//! arch-SA §2.2, §9 coverage row for these three requirements).
//!
//! This module owns exactly three concerns, none of which overlap with `merge.rs` (four-tier
//! precedence merge, not yet written as of this file) or `frontmatter.rs` (parse-only, already
//! written and reused here read-only for round-trip verification):
//!
//! 1. **Read-only-source rejection (R-SA-014).** A create/update/delete/rename targeting a
//!    `Builtin`- or `Package`-sourced agent or chain MUST fail with
//!    [`crate::error::SubagentError::ReadOnlySource`] before any filesystem mutation is
//!    attempted. Only `User`/`Project`-sourced files are writable through this module.
//! 2. **Call-site-dependent `disabled` visibility (R-SA-013).** Three independently testable
//!    views over "the same" underlying agent/chain set:
//!    - [`AgentVisibility::management`] — full, unfiltered (used for CRUD and re-enabling); MUST
//!      include disabled agents.
//!    - [`AgentVisibility::delegation`] — runtime-filtered; MUST exclude disabled agents, since
//!      this is the view actual execution-time selection uses.
//!    - [`AgentVisibility::list`] — filtered independently of the other two (a human-facing list
//!      view defaults to hiding disabled agents but is a *distinct* code path from delegation's
//!      filter, not a shared implementation detail masquerading as two call sites — see that
//!      function's own doc for why it is kept textually separate from `delegation` even though
//!      both currently apply the same predicate).
//! 3. **On-demand, re-scanned-per-call semantics (R-SA-019).** This module holds no cache and no
//!    filesystem watcher; every function here operates on a caller-supplied `&[AgentDefinition]`/
//!    `&[ChainDefinition]` snapshot (the caller — `discovery/mod.rs`'s entry points, once written
//!    — is responsible for re-invoking discovery before each mutating call in a create -> get ->
//!    update -> delete sequence, per R-SA-019's own text: *"Callers that need up-to-date state
//!    across a sequence of management actions... MUST re-invoke discovery before each mutating
//!    action rather than reusing a cached result."* This module does not and cannot violate that
//!    on its own — it simply never introduces a cache to violate it with.
//!
//! # Deferred to later phases (explicitly, per this task's own instructions)
//!
//! - **`merge.rs`** (four-tier Builtin/Package/User/Project precedence merge, R-SA-001/002) is a
//!   sibling file owned by a later/concurrent phase. This module does not merge scopes; it
//!   operates on a flat, already-scoped `&[AgentDefinition]` slice the caller assembled (from
//!   discovery or from a targeted single-scope directory scan) and only needs each entry's
//!   `source`/`name`/`file_path` fields, which `AgentDefinition` already provides regardless of
//!   whether `merge.rs` exists yet.
//! - **`discovery/mod.rs`'s `discover_agents_all`/`discover_agents` entry points** (R-SA-001..004
//!   directory-walk orchestration) are likewise a later phase. This module's CRUD functions take
//!   an explicit `scope_dir: &Path` parameter for exactly the scope (`User`/`Project`) being
//!   mutated, rather than re-deriving cyrup's config-directory resolution itself — that
//!   resolution is `discovery/mod.rs`'s job, not this file's.
//! - **Chain-file management** reuses [`crate::discovery::chains`]'s already-written
//!   `.chain.json`-over-`.chain.md` same-name precedence (R-SA-015) purely as a read-side
//!   discovery helper for `list`/`get`-style callers; this module's own chain CRUD writes
//!   `.chain.json` exclusively (the plain-`serde_json` format, since it has no reason to prefer
//!   the frontmatter-grammar `.chain.md` format when authoring new content) and never attempts to
//!   mutate an existing `.chain.md` file in place — a caller renaming/updating a `.chain.md`-authored
//!   chain gets a fresh `.chain.json` file at the same logical name, which (per R-SA-015) then
//!   takes over same-directory precedence on the next discovery pass.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use cyrup_core::{ModelId, ThinkingLevel};

use crate::error::SubagentError;
use crate::fork_context::ContextMode;
use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};

use super::types::{
    AgentDefinition, AgentSource, ChainDefinition, OutputSpec, SystemPromptMode, ToolRef,
};

// -------------------------------------------------------------------------------------------
// R-SA-013: call-site-dependent `disabled` visibility
// -------------------------------------------------------------------------------------------

/// Three independently testable views over the *same* underlying agent set, differing only in
/// how `AgentDefinition::disabled` is treated (R-SA-013). Each is exposed as its own function
/// (rather than one function taking a boolean) so call sites are self-documenting and so a future
/// change to any one view's semantics cannot accidentally also change another's.
pub struct AgentVisibility;

impl AgentVisibility {
    /// Management/introspection listing: used for CRUD and re-enabling. MUST include disabled
    /// agents (R-SA-013) — a caller needs to *see* a disabled agent in order to re-enable it, so
    /// this view is deliberately unfiltered. Returns every entry in `agents`, in the same order.
    pub fn management(agents: &[AgentDefinition]) -> Vec<&AgentDefinition> {
        agents.iter().collect()
    }

    /// Delegation/execution-time selection: the view actual runtime dispatch uses to resolve a
    /// requested agent name. MUST exclude disabled agents (R-SA-013) — a disabled agent is not a
    /// valid delegation target regardless of how it is named.
    pub fn delegation(agents: &[AgentDefinition]) -> Vec<&AgentDefinition> {
        agents
            .iter()
            .filter(|a| !a.disabled.unwrap_or(false))
            .collect()
    }

    /// Human-facing list view (e.g. a `/subagents-list`-style command's default rendering).
    /// Filtered independently of [`Self::delegation`] (R-SA-013's "these are two distinct,
    /// independently testable behaviors" framing extends to keeping this call site textually
    /// separate from delegation's, not merely reusing its result under a different name) — a
    /// list view legitimately might diverge from delegation's filter in the future (e.g. gaining
    /// a `--show-disabled` flag that flips only this view's predicate without touching
    /// delegation's), so the two are never collapsed into a single shared function even though
    /// their current predicate is identical.
    pub fn list(agents: &[AgentDefinition]) -> Vec<&AgentDefinition> {
        agents
            .iter()
            .filter(|a| !a.disabled.unwrap_or(false))
            .collect()
    }
}

/// Chain-definition analog of [`AgentVisibility`]. `ChainDefinition` (func-SA §4.1) has no
/// `disabled` field of its own in the current data model, so all three views are currently
/// identical (unfiltered) passthroughs — kept as distinct functions for the same
/// forward-compatibility reason as [`AgentVisibility::list`] above, and so call sites read
/// identically to their agent counterparts.
pub struct ChainVisibility;

impl ChainVisibility {
    pub fn management(chains: &[ChainDefinition]) -> Vec<&ChainDefinition> {
        chains.iter().collect()
    }

    pub fn delegation(chains: &[ChainDefinition]) -> Vec<&ChainDefinition> {
        chains.iter().collect()
    }

    pub fn list(chains: &[ChainDefinition]) -> Vec<&ChainDefinition> {
        chains.iter().collect()
    }
}

// -------------------------------------------------------------------------------------------
// R-SA-014: read-only-source guard
// -------------------------------------------------------------------------------------------

/// Reject a management operation targeting a non-writable [`AgentSource`] (R-SA-014). Called
/// first, before any filesystem access, by every mutating function in this module.
fn require_writable_source(source: AgentSource, target_name: &str) -> Result<(), SubagentError> {
    if source.is_writable() {
        Ok(())
    } else {
        Err(SubagentError::ReadOnlySource(target_name.to_string()))
    }
}

// -------------------------------------------------------------------------------------------
// Agent field-set: the caller-supplied delta for create/update (management-facing shape,
// deliberately narrower than the full AgentDefinition — present_fields/extra_fields/source/
// file_path/override_info/model_source are either derived or not caller-settable here).
// -------------------------------------------------------------------------------------------

/// The subset of [`AgentDefinition`] fields a management create/update call may supply. `None`
/// on an `Option<T>`-typed field of this struct means "caller did not touch this field" (for
/// `update_agent`, the existing on-disk value is preserved; for `create_agent`, the
/// [`AgentDefinition`]-level default applies). This is deliberately **not** the three-state
/// [`super::types::OverrideField`] shape — that type exists for *settings-layer* override deltas
/// (R-SA-011), a different concept from an interactive/API-driven management edit, which has no
/// "explicit clear" sentinel requirement of its own (a management caller who wants to clear a
/// field passes an explicit empty/None value for it, same as any ordinary struct-update API).
#[derive(Clone, Debug, Default)]
pub struct AgentFields {
    pub local_name: Option<String>,
    pub package_name: Option<Option<String>>,
    pub description: Option<String>,
    pub tools: Option<Option<Vec<ToolRef>>>,
    pub extensions: Option<Option<Vec<String>>>,
    pub subagent_only_extensions: Option<Vec<String>>,
    pub model: Option<Option<ModelId>>,
    pub fallback_models: Option<Vec<ModelId>>,
    pub thinking: Option<Option<ThinkingLevel>>,
    pub system_prompt_mode: Option<SystemPromptMode>,
    pub inherit_project_context: Option<bool>,
    pub inherit_skills: Option<bool>,
    pub skills: Option<Vec<String>>,
    pub default_reads: Option<Option<Vec<PathBuf>>>,
    pub default_progress: Option<Option<bool>>,
    pub output: Option<Option<OutputSpec>>,
    pub completion_guard: Option<Option<bool>>,
    pub interactive: Option<Option<bool>>,
    pub max_subagent_depth: Option<Option<u32>>,
    pub default_context: Option<Option<ContextMode>>,
    pub disabled: Option<Option<bool>>,
    pub system_prompt_body: Option<String>,
}

/// The outcome of a successful management mutation: the resulting [`AgentDefinition`] as it now
/// exists on disk (re-parsed from the freshly-written file, so callers observe exactly what a
/// subsequent discovery pass would see — never a synthesized in-memory value that might drift
/// from the on-disk round-trip).
#[derive(Debug)]
pub struct AgentMutationOutcome {
    pub definition: AgentDefinition,
    pub file_path: PathBuf,
}

// -------------------------------------------------------------------------------------------
// Package-identifier validation (mirrors `frontmatter.rs::parse_package_name`'s validation
// grammar exactly, R-SA-006). Duplicated locally (rather than importing a private helper from
// `frontmatter.rs`) since this module owns its own file and must not require edits to
// `frontmatter.rs` to build; the two implementations are each unit-tested against the same
// fixture set to guard against drift.
// -------------------------------------------------------------------------------------------

/// Normalize + validate a caller-supplied package identifier exactly per R-SA-006's grammar:
/// lowercase, whitespace runs -> `-`, strip anything outside `[a-z0-9.-]`, collapse repeated
/// `-`/`.` runs, trim leading/trailing `-`/`.`, then require
/// `^[a-z0-9][a-z0-9-]*(?:\.[a-z0-9][a-z0-9-]*)*$`.
///
/// Returns `Ok(None)` for an absent/empty/whitespace-only input (not a validation failure).
/// Returns `Ok(None)` — **not** `Err` — for a non-empty input that fails to normalize to a valid
/// identifier: per this module's own "invalid package identifier -> silent skip, not an error"
/// contract (R-SA-004/011 taxonomy note: discovery's per-file skip behavior for this exact
/// condition, R-SA-006, is mirrored here rather than promoted to a hard management-layer error),
/// callers that receive `Ok(None)` from a caller-supplied non-empty package value MUST treat the
/// whole create/update call as skipped (a no-op returning `Ok(None)` at the call-site level, see
/// [`create_agent`]/[`update_agent`]) rather than surfacing a `SubagentError`.
fn normalize_package_identifier(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lowered = trimmed.to_lowercase();
    let mut collapsed_ws = String::with_capacity(lowered.len());
    let mut last_was_ws = false;
    for ch in lowered.chars() {
        if ch.is_whitespace() {
            if !last_was_ws {
                collapsed_ws.push('-');
            }
            last_was_ws = true;
        } else {
            collapsed_ws.push(ch);
            last_was_ws = false;
        }
    }
    let filtered: String = collapsed_ws
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let collapsed_hyphen = collapse_repeated_char(&filtered, '-');
    let collapsed_dot = collapse_repeated_char(&collapsed_hyphen, '.');
    let final_name = collapsed_dot
        .trim_start_matches(['-', '.'])
        .trim_end_matches(['-', '.'])
        .to_string();

    if final_name.is_empty() || !is_valid_package_identifier(&final_name) {
        return None;
    }
    Some(final_name)
}

fn collapse_repeated_char(s: &str, target: char) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_was_target = false;
    for ch in s.chars() {
        if ch == target {
            if !prev_was_target {
                out.push(ch);
            }
            prev_was_target = true;
        } else {
            out.push(ch);
            prev_was_target = false;
        }
    }
    out
}

fn is_valid_package_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    for segment in s.split('.') {
        let mut chars = segment.chars();
        match chars.next() {
            Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
            _ => return false,
        }
        if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return false;
        }
    }
    true
}

// -------------------------------------------------------------------------------------------
// Agent create/update/delete/rename
// -------------------------------------------------------------------------------------------

/// Create a new User/Project-scope agent file under `scope_dir` (a caller-resolved, already
/// scope-specific directory — this module does not itself resolve `AgentReadScope` to a
/// filesystem path; that is `discovery/mod.rs`'s job).
///
/// Returns:
/// - `Ok(Some(outcome))` on success.
/// - `Ok(None)` if `fields.package_name` was supplied as `Some(Some(raw))` and `raw` fails
///   R-SA-006's package-identifier validation — a **silent skip**, matching discovery's own
///   whole-file-skip behavior for the identical malformed-package-identifier condition, never an
///   `Err` (per this module's own taxonomy note above and the task's R-SA-004/011 test
///   requirement).
/// - `Err(SubagentError::ReadOnlySource)` if `source` is `Builtin`/`Package` (R-SA-014).
/// - `Err(SubagentError::Spawn)` (via `#[from] std::io::Error`) on a genuine filesystem failure
///   (permission denied, disk full, etc.) — distinct from the silent-skip case above, which is
///   not a filesystem failure at all.
pub fn create_agent(
    scope_dir: &Path,
    source: AgentSource,
    local_name: &str,
    description: &str,
    fields: &AgentFields,
) -> Result<Option<AgentMutationOutcome>, SubagentError> {
    require_writable_source(source, local_name)?;

    let package_name = match &fields.package_name {
        Some(Some(raw)) => match normalize_package_identifier(Some(raw)) {
            Some(normalized) => Some(normalized),
            None => return Ok(None), // R-SA-006 silent skip, mirrored at the management layer.
        },
        Some(None) => None,
        None => None,
    };

    let definition = build_definition(
        source,
        scope_dir,
        local_name,
        package_name,
        description,
        fields,
    );

    let file_path = agent_file_path(scope_dir, local_name);
    if file_path.exists() {
        return Err(SubagentError::Spawn(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("agent file already exists: {}", file_path.display()),
        )));
    }

    write_agent_file(&file_path, &definition)?;
    let reparsed = reparse_agent_file(&file_path, source)?;
    Ok(Some(AgentMutationOutcome {
        definition: reparsed,
        file_path,
    }))
}

/// Update an existing User/Project-scope agent in place, applying only the fields explicitly set
/// on `fields` (an `Option`-typed field left `None` on `fields` preserves `existing`'s current
/// value — this is a plain field-level patch, not R-SA-010's fill-unset-only *settings-override*
/// semantics, which is a different mechanism entirely owned by `merge.rs`).
///
/// Same three-way return contract as [`create_agent`]: `Ok(Some(_))` on success, `Ok(None)` on a
/// silent package-identifier-validation skip (R-SA-006), `Err(ReadOnlySource)` for a
/// Builtin/Package target (R-SA-014).
pub fn update_agent(
    existing: &AgentDefinition,
    fields: &AgentFields,
) -> Result<Option<AgentMutationOutcome>, SubagentError> {
    require_writable_source(existing.source, &existing.name)?;

    let package_name = match &fields.package_name {
        Some(Some(raw)) => match normalize_package_identifier(Some(raw)) {
            Some(normalized) => Some(normalized),
            None => return Ok(None), // R-SA-006 silent skip.
        },
        Some(None) => None,
        None => existing.package_name.clone(),
    };

    let local_name = fields
        .local_name
        .clone()
        .unwrap_or_else(|| existing.local_name.clone());
    let description = fields
        .description
        .clone()
        .unwrap_or_else(|| existing.description.clone());

    let merged = merge_fields(existing, &local_name, package_name, &description, fields);

    write_agent_file(&existing.file_path, &merged)?;
    let reparsed = reparse_agent_file(&existing.file_path, existing.source)?;
    Ok(Some(AgentMutationOutcome {
        definition: reparsed,
        file_path: existing.file_path.clone(),
    }))
}

/// Delete a User/Project-scope agent's on-disk file. Fails with `ReadOnlySource` for a
/// Builtin/Package target (R-SA-014) *before* attempting any filesystem removal.
pub fn delete_agent(existing: &AgentDefinition) -> Result<(), SubagentError> {
    require_writable_source(existing.source, &existing.name)?;
    std::fs::remove_file(&existing.file_path).map_err(SubagentError::Spawn)?;
    Ok(())
}

/// Rename a User/Project-scope agent: writes the (unchanged-content) frontmatter under a new
/// `local_name`/file path within the same scope directory, then removes the old file. If the
/// rename would collide with an existing file at the destination path, no mutation occurs and an
/// `AlreadyExists`-kind `SubagentError::Spawn` is returned. Fails with `ReadOnlySource` for a
/// Builtin/Package target (R-SA-014) before any filesystem access.
pub fn rename_agent(
    existing: &AgentDefinition,
    new_local_name: &str,
) -> Result<AgentMutationOutcome, SubagentError> {
    require_writable_source(existing.source, &existing.name)?;

    let scope_dir = existing
        .file_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let new_path = agent_file_path(&scope_dir, new_local_name);

    if new_path == existing.file_path {
        // Renaming to the same name is a no-op success (idempotent), not an error.
        return Ok(AgentMutationOutcome {
            definition: existing.clone(),
            file_path: existing.file_path.clone(),
        });
    }
    if new_path.exists() {
        return Err(SubagentError::Spawn(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("agent file already exists: {}", new_path.display()),
        )));
    }

    let mut renamed = existing.clone();
    renamed.local_name = new_local_name.to_string();
    renamed.name = AgentDefinition::qualified_name(new_local_name, existing.package_name.as_deref());
    renamed.file_path = new_path.clone();

    write_agent_file(&new_path, &renamed)?;
    std::fs::remove_file(&existing.file_path).map_err(SubagentError::Spawn)?;

    let reparsed = reparse_agent_file(&new_path, existing.source)?;
    Ok(AgentMutationOutcome {
        definition: reparsed,
        file_path: new_path,
    })
}

fn agent_file_path(scope_dir: &Path, local_name: &str) -> PathBuf {
    scope_dir.join(format!("{local_name}.md"))
}

fn build_definition(
    source: AgentSource,
    scope_dir: &Path,
    local_name: &str,
    package_name: Option<String>,
    description: &str,
    fields: &AgentFields,
) -> AgentDefinition {
    let runtime_name = AgentDefinition::qualified_name(local_name, package_name.as_deref());
    AgentDefinition {
        name: runtime_name,
        local_name: local_name.to_string(),
        package_name,
        description: description.to_string(),
        tools: fields.tools.clone().unwrap_or(None),
        extensions: fields.extensions.clone().unwrap_or(None),
        subagent_only_extensions: fields.subagent_only_extensions.clone().unwrap_or_default(),
        model: fields.model.clone().unwrap_or(None),
        fallback_models: fields.fallback_models.clone().unwrap_or_default(),
        thinking: fields.thinking.unwrap_or(None),
        system_prompt_mode: fields.system_prompt_mode.unwrap_or(SystemPromptMode::Replace),
        inherit_project_context: fields.inherit_project_context.unwrap_or(false),
        inherit_skills: fields.inherit_skills.unwrap_or(false),
        skills: fields.skills.clone().unwrap_or_default(),
        default_reads: fields.default_reads.clone().unwrap_or(None),
        default_progress: fields.default_progress.unwrap_or(None),
        output: fields.output.clone().unwrap_or(None),
        completion_guard: fields.completion_guard.unwrap_or(None),
        interactive: fields.interactive.unwrap_or(None),
        max_subagent_depth: fields.max_subagent_depth.unwrap_or(None),
        default_context: fields.default_context.unwrap_or(None),
        disabled: fields.disabled.unwrap_or(None),
        system_prompt_body: fields.system_prompt_body.clone().unwrap_or_default(),
        source,
        file_path: agent_file_path(scope_dir, local_name),
        present_fields: HashSet::new(),
        extra_fields: BTreeMap::new(),
        override_info: None,
        model_source: None,
    }
}

fn merge_fields(
    existing: &AgentDefinition,
    local_name: &str,
    package_name: Option<String>,
    description: &str,
    fields: &AgentFields,
) -> AgentDefinition {
    let runtime_name = AgentDefinition::qualified_name(local_name, package_name.as_deref());
    AgentDefinition {
        name: runtime_name,
        local_name: local_name.to_string(),
        package_name,
        description: description.to_string(),
        tools: fields.tools.clone().unwrap_or_else(|| existing.tools.clone()),
        extensions: fields
            .extensions
            .clone()
            .unwrap_or_else(|| existing.extensions.clone()),
        subagent_only_extensions: fields
            .subagent_only_extensions
            .clone()
            .unwrap_or_else(|| existing.subagent_only_extensions.clone()),
        model: fields.model.clone().unwrap_or_else(|| existing.model.clone()),
        fallback_models: fields
            .fallback_models
            .clone()
            .unwrap_or_else(|| existing.fallback_models.clone()),
        thinking: fields.thinking.unwrap_or(existing.thinking),
        system_prompt_mode: fields.system_prompt_mode.unwrap_or(existing.system_prompt_mode),
        inherit_project_context: fields
            .inherit_project_context
            .unwrap_or(existing.inherit_project_context),
        inherit_skills: fields.inherit_skills.unwrap_or(existing.inherit_skills),
        skills: fields.skills.clone().unwrap_or_else(|| existing.skills.clone()),
        default_reads: fields
            .default_reads
            .clone()
            .unwrap_or_else(|| existing.default_reads.clone()),
        default_progress: fields.default_progress.unwrap_or(existing.default_progress),
        output: fields.output.clone().unwrap_or_else(|| existing.output.clone()),
        completion_guard: fields.completion_guard.unwrap_or(existing.completion_guard),
        interactive: fields.interactive.unwrap_or(existing.interactive),
        max_subagent_depth: fields.max_subagent_depth.unwrap_or(existing.max_subagent_depth),
        default_context: fields.default_context.unwrap_or(existing.default_context),
        disabled: fields.disabled.unwrap_or(existing.disabled),
        system_prompt_body: fields
            .system_prompt_body
            .clone()
            .unwrap_or_else(|| existing.system_prompt_body.clone()),
        source: existing.source,
        file_path: existing.file_path.clone(),
        present_fields: existing.present_fields.clone(),
        extra_fields: existing.extra_fields.clone(),
        override_info: existing.override_info.clone(),
        model_source: existing.model_source,
    }
}

/// Re-read and re-parse the just-written file via [`crate::discovery::frontmatter::parse_agent_file`]
/// so callers observe exactly what a subsequent discovery pass would see (never a synthesized
/// in-memory value that might drift from the on-disk round-trip). A `None` result here (which
/// would mean the file this module itself just wrote fails discovery's own required-field check)
/// indicates an internal serialization bug in [`write_agent_file`], surfaced as a `Spawn`-flavored
/// I/O error rather than silently returning a definition that does not match what was written.
fn reparse_agent_file(file_path: &Path, source: AgentSource) -> Result<AgentDefinition, SubagentError> {
    let content = std::fs::read_to_string(file_path).map_err(SubagentError::Spawn)?;
    super::frontmatter::parse_agent_file(&content, source, file_path).ok_or_else(|| {
        SubagentError::Spawn(std::io::Error::other(format!(
            "internal error: just-written agent file at {} failed to re-parse",
            file_path.display()
        )))
    })
}

// -------------------------------------------------------------------------------------------
// Frontmatter serialization (write-back). A minimal, deliberately narrow inverse of
// `frontmatter.rs`'s parse grammar: flat `key: value` lines only (this module never needs to
// write a block-indent value, since none of `AgentFields`' caller-settable fields require one —
// `extra_fields`/unknown-key round-trip on update is preserved verbatim from `existing` via
// `merge_fields` above, and re-serialized here using the same flat-line writer, which is
// sufficient because `parse_frontmatter_block`'s block-continuation grammar only activates for
// an *empty-valued* key, never for a key whose value the writer supplies non-empty).
// -------------------------------------------------------------------------------------------

fn write_agent_file(file_path: &Path, definition: &AgentDefinition) -> Result<(), SubagentError> {
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(SubagentError::Spawn)?;
    }
    let content = serialize_agent_frontmatter(definition);
    std::fs::write(file_path, content).map_err(SubagentError::Spawn)?;
    Ok(())
}

fn serialize_agent_frontmatter(def: &AgentDefinition) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("---".to_string());
    lines.push(format!("name: {}", def.local_name));
    if let Some(pkg) = &def.package_name {
        lines.push(format!("package: {pkg}"));
    }
    lines.push(format!("description: {}", def.description));

    if let Some(tools) = &def.tools {
        let joined = tools
            .iter()
            .map(tool_ref_to_frontmatter_entry)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("tools: {joined}"));
    }
    if let Some(model) = &def.model {
        lines.push(format!("model: {model}"));
    }
    if !def.fallback_models.is_empty() {
        let joined = def
            .fallback_models
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("fallbackModels: {joined}"));
    }
    if let Some(thinking) = def.thinking {
        lines.push(format!("thinking: {}", thinking_level_to_str(thinking)));
    }
    lines.push(format!(
        "systemPromptMode: {}",
        match def.system_prompt_mode {
            SystemPromptMode::Append => "append",
            SystemPromptMode::Replace => "replace",
        }
    ));
    lines.push(format!(
        "inheritProjectContext: {}",
        def.inherit_project_context
    ));
    lines.push(format!("inheritSkills: {}", def.inherit_skills));
    if !def.skills.is_empty() {
        lines.push(format!("skills: {}", def.skills.join(", ")));
    }
    if let Some(exts) = &def.extensions {
        lines.push(format!("extensions: {}", exts.join(", ")));
    }
    if !def.subagent_only_extensions.is_empty() {
        lines.push(format!(
            "subagentOnlyExtensions: {}",
            def.subagent_only_extensions.join(", ")
        ));
    }
    if let Some(output) = &def.output
        && let Some(path) = &output.path
    {
        lines.push(format!("output: {}", path.display()));
    }
    if let Some(reads) = &def.default_reads {
        let joined = reads
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("defaultReads: {joined}"));
    }
    if let Some(progress) = def.default_progress {
        lines.push(format!("defaultProgress: {progress}"));
    }
    if let Some(interactive) = def.interactive {
        lines.push(format!("interactive: {interactive}"));
    }
    if let Some(depth) = def.max_subagent_depth {
        lines.push(format!("maxSubagentDepth: {depth}"));
    }
    if let Some(guard) = def.completion_guard {
        lines.push(format!("completionGuard: {guard}"));
    }
    if let Some(ctx) = def.default_context {
        lines.push(format!(
            "defaultContext: {}",
            match ctx {
                ContextMode::Fresh => "fresh",
                ContextMode::Fork => "fork",
            }
        ));
    }
    if let Some(disabled) = def.disabled {
        lines.push(format!("disabled: {disabled}"));
    }
    // Unknown-key round-trip: re-emit every `extra_fields` entry verbatim as a flat line. Since
    // this writer never produces block-indent values, an `extra_fields` value that itself
    // contains embedded newlines (originally captured from a block value by `frontmatter.rs`) is
    // written back as a single flat line with literal embedded newlines preserved in the string —
    // `parse_frontmatter_block`'s re-read of this file will treat everything after the first
    // newline as new (likely non-matching, silently-ignored) lines rather than reconstructing the
    // original block. This is a known, narrow round-trip gap for the block-value case
    // specifically; flat extra_fields values (the common case) round-trip exactly. Widening this
    // to re-emit true block-indent values is deferred to a later phase alongside `merge.rs`'s own
    // override-serialization needs, since both would share the same block-emission helper.
    for (key, value) in &def.extra_fields {
        lines.push(format!("{key}: {value}"));
    }

    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(def.system_prompt_body.clone());
    lines.push(String::new());
    lines.join("\n")
}

fn tool_ref_to_frontmatter_entry(tool: &ToolRef) -> String {
    match tool {
        ToolRef::Builtin(name) | ToolRef::ExtensionPath(name) => name.clone(),
        ToolRef::Mcp(name) => format!("mcp:{name}"),
    }
}

fn thinking_level_to_str(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
    }
}

// -------------------------------------------------------------------------------------------
// Chain create/update/delete/rename (R-SA-014 applies identically to chains)
// -------------------------------------------------------------------------------------------

/// The caller-supplied delta for a chain create/update — deliberately minimal. This module's own
/// job (R-SA-013/014/019 CRUD + visibility) does not include full chain-step authoring (building
/// up a real `RunnerStep::SingleStep`/`ParallelGroup`/`DynamicGroup` sequence is a chain-editor
/// concern, not a bare-CRUD concern) — `step_count` only controls how many placeholder
/// [`RunnerStep::SingleStep`] entries this module materializes when it needs to preserve or
/// resize a chain's step list without inventing per-step content of its own. A future
/// chain-editor-facing API (outside this file's R-SA-013/014/019 scope) would supply real
/// [`RunnerStep`] values directly rather than going through `step_count`.
#[derive(Clone, Debug, Default)]
pub struct ChainFields {
    pub name: Option<String>,
    pub description: Option<String>,
    pub step_count: Option<usize>,
}

/// Build one minimal, valid placeholder [`RunnerStep::SingleStep`] — used only to preserve step
/// *count* across a management-layer chain update that does not itself author step content (see
/// [`ChainFields`]'s own doc). Every field left at its "no override" default so this placeholder
/// carries no spurious behavior if ever (mis)dispatched directly.
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

/// Create a new `.chain.json` file under `scope_dir` (R-SA-014: `source` must be `User`/
/// `Project`). Chain names have no package-identifier concept (R-SA-006 does not apply to
/// chains), so unlike [`create_agent`] this function has no silent-skip return path — it either
/// succeeds or returns a hard `Err`.
pub fn create_chain(
    scope_dir: &Path,
    source: AgentSource,
    name: &str,
    description: &str,
) -> Result<ChainDefinition, SubagentError> {
    require_writable_source(source, name)?;

    let file_path = chain_file_path(scope_dir, name);
    if file_path.exists() {
        return Err(SubagentError::Spawn(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("chain file already exists: {}", file_path.display()),
        )));
    }

    let definition = ChainDefinition {
        name: name.to_string(),
        description: description.to_string(),
        source,
        file_path: file_path.clone(),
        steps: Vec::new(),
    };

    write_chain_file(&file_path, &definition)?;
    Ok(definition)
}

/// Update an existing User/Project-scope chain's `name`/`description`/step count in place.
/// Fails with `ReadOnlySource` for a Builtin/Package target (R-SA-014).
pub fn update_chain(
    existing: &ChainDefinition,
    fields: &ChainFields,
) -> Result<ChainDefinition, SubagentError> {
    require_writable_source(existing.source, &existing.name)?;

    let name = fields.name.clone().unwrap_or_else(|| existing.name.clone());
    let description = fields
        .description
        .clone()
        .unwrap_or_else(|| existing.description.clone());
    let step_count = fields.step_count.unwrap_or(existing.steps.len());

    let updated = ChainDefinition {
        name,
        description,
        source: existing.source,
        file_path: existing.file_path.clone(),
        steps: (0..step_count).map(|_| placeholder_runner_step()).collect(),
    };

    write_chain_file(&existing.file_path, &updated)?;
    Ok(updated)
}

/// Delete a User/Project-scope chain's on-disk file. Fails with `ReadOnlySource` for a
/// Builtin/Package target (R-SA-014) before any filesystem removal.
pub fn delete_chain(existing: &ChainDefinition) -> Result<(), SubagentError> {
    require_writable_source(existing.source, &existing.name)?;
    std::fs::remove_file(&existing.file_path).map_err(SubagentError::Spawn)?;
    Ok(())
}

/// Rename a User/Project-scope chain to `new_name`, writing a fresh `.chain.json` at the new path
/// (per this module's own header note: chain CRUD always (re)writes `.chain.json`, even when the
/// original file was a `.chain.md`) and removing the old file. Fails with `ReadOnlySource` for a
/// Builtin/Package target (R-SA-014) before any filesystem access.
pub fn rename_chain(
    existing: &ChainDefinition,
    new_name: &str,
) -> Result<ChainDefinition, SubagentError> {
    require_writable_source(existing.source, &existing.name)?;

    let scope_dir = existing
        .file_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let new_path = chain_file_path(&scope_dir, new_name);

    if new_path == existing.file_path {
        return Ok(existing.clone());
    }
    if new_path.exists() {
        return Err(SubagentError::Spawn(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("chain file already exists: {}", new_path.display()),
        )));
    }

    let renamed = ChainDefinition {
        name: new_name.to_string(),
        description: existing.description.clone(),
        source: existing.source,
        file_path: new_path.clone(),
        steps: existing.steps.clone(),
    };

    write_chain_file(&new_path, &renamed)?;
    std::fs::remove_file(&existing.file_path).map_err(SubagentError::Spawn)?;
    Ok(renamed)
}

fn chain_file_path(scope_dir: &Path, name: &str) -> PathBuf {
    scope_dir.join(format!("{name}.chain.json"))
}

fn write_chain_file(file_path: &Path, definition: &ChainDefinition) -> Result<(), SubagentError> {
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(SubagentError::Spawn)?;
    }
    let content = serialize_chain_json(definition);
    std::fs::write(file_path, content).map_err(SubagentError::Spawn)?;
    Ok(())
}

/// Serialize a [`ChainDefinition`] to the plain-`serde_json` `.chain.json` shape
/// [`crate::discovery::chains::parse_chain_json`] reads. `RunnerStep` now has a real, tagged
/// `Serialize` impl (`spawn/chain_graph.rs`), so each step is emitted as its actual
/// `{"kind": "singleStep" | "parallelGroup" | "dynamicGroup", ...}` payload rather than an opaque
/// placeholder — `discovery/chains.rs`'s reader-side `parse_chain_json_steps` still only counts
/// array elements as of this file (see that module's own "Deferred: full `RunnerStep` field
/// population" note), so round-tripping full step content through a read-back discovery pass is
/// still gated on that sibling file's own later update, but this module's write side no longer
/// discards real step content itself.
fn serialize_chain_json(def: &ChainDefinition) -> String {
    let steps: Vec<serde_json::Value> = def
        .steps
        .iter()
        .map(|step| serde_json::to_value(step).unwrap_or_else(|_| serde_json::json!({})))
        .collect();
    let value = serde_json::json!({
        "name": def.name,
        "description": def.description,
        "steps": steps,
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn sample_agent(source: AgentSource, file_path: PathBuf) -> AgentDefinition {
        AgentDefinition {
            name: "reviewer".to_string(),
            local_name: "reviewer".to_string(),
            package_name: None,
            description: "reviews things".to_string(),
            tools: None,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            model: None,
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: SystemPromptMode::Replace,
            inherit_project_context: false,
            inherit_skills: false,
            skills: Vec::new(),
            default_reads: None,
            default_progress: None,
            output: None,
            completion_guard: None,
            interactive: None,
            max_subagent_depth: None,
            default_context: None,
            disabled: None,
            system_prompt_body: "You are a reviewer.".to_string(),
            source,
            file_path,
            present_fields: HashSet::new(),
            extra_fields: BTreeMap::new(),
            override_info: None,
            model_source: None,
        }
    }

    fn sample_chain(source: AgentSource, file_path: PathBuf) -> ChainDefinition {
        ChainDefinition {
            name: "release".to_string(),
            description: "release chain".to_string(),
            source,
            file_path,
            steps: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-014: read-only-source rejection for Builtin/Package targets (agents)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn create_agent_rejects_builtin_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = create_agent(
            tmp.path(),
            AgentSource::Builtin,
            "scout",
            "desc",
            &AgentFields::default(),
        );
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
        // No filesystem mutation occurred.
        assert!(!tmp.path().join("scout.md").exists());
    }

    #[test]
    fn create_agent_rejects_package_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = create_agent(
            tmp.path(),
            AgentSource::Package,
            "scout",
            "desc",
            &AgentFields::default(),
        );
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
    }

    #[test]
    fn create_agent_succeeds_for_user_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outcome = create_agent(
            tmp.path(),
            AgentSource::User,
            "scout",
            "Fast recon",
            &AgentFields::default(),
        )
        .expect("no error")
        .expect("not silently skipped");
        assert_eq!(outcome.definition.name, "scout");
        assert_eq!(outcome.definition.description, "Fast recon");
        assert!(outcome.file_path.exists());
    }

    #[test]
    fn create_agent_succeeds_for_project_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outcome = create_agent(
            tmp.path(),
            AgentSource::Project,
            "scout",
            "Fast recon",
            &AgentFields::default(),
        )
        .expect("no error")
        .expect("not silently skipped");
        assert_eq!(outcome.definition.source, AgentSource::Project);
    }

    #[test]
    fn update_agent_rejects_builtin_source() {
        let existing = sample_agent(AgentSource::Builtin, PathBuf::from("/builtin/reviewer.md"));
        let result = update_agent(&existing, &AgentFields::default());
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
    }

    #[test]
    fn update_agent_rejects_package_source() {
        let existing = sample_agent(AgentSource::Package, PathBuf::from("/pkg/reviewer.md"));
        let result = update_agent(&existing, &AgentFields::default());
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
    }

    #[test]
    fn update_agent_succeeds_for_project_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let created = create_agent(
            tmp.path(),
            AgentSource::Project,
            "reviewer",
            "old desc",
            &AgentFields::default(),
        )
        .expect("no error")
        .expect("not skipped");

        let fields = AgentFields {
            description: Some("new desc".to_string()),
            ..AgentFields::default()
        };
        let updated = update_agent(&created.definition, &fields)
            .expect("no error")
            .expect("not skipped");
        assert_eq!(updated.definition.description, "new desc");
    }

    #[test]
    fn delete_agent_rejects_builtin_source() {
        let existing = sample_agent(AgentSource::Builtin, PathBuf::from("/builtin/reviewer.md"));
        let result = delete_agent(&existing);
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
    }

    #[test]
    fn delete_agent_rejects_package_source() {
        let existing = sample_agent(AgentSource::Package, PathBuf::from("/pkg/reviewer.md"));
        let result = delete_agent(&existing);
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
    }

    #[test]
    fn delete_agent_succeeds_for_user_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let created = create_agent(
            tmp.path(),
            AgentSource::User,
            "reviewer",
            "desc",
            &AgentFields::default(),
        )
        .expect("no error")
        .expect("not skipped");
        assert!(created.file_path.exists());

        delete_agent(&created.definition).expect("delete succeeds");
        assert!(!created.file_path.exists());
    }

    #[test]
    fn rename_agent_rejects_builtin_source() {
        let existing = sample_agent(AgentSource::Builtin, PathBuf::from("/builtin/reviewer.md"));
        let result = rename_agent(&existing, "renamed");
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
    }

    #[test]
    fn rename_agent_rejects_package_source() {
        let existing = sample_agent(AgentSource::Package, PathBuf::from("/pkg/reviewer.md"));
        let result = rename_agent(&existing, "renamed");
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
    }

    #[test]
    fn rename_agent_succeeds_for_user_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let created = create_agent(
            tmp.path(),
            AgentSource::User,
            "reviewer",
            "desc",
            &AgentFields::default(),
        )
        .expect("no error")
        .expect("not skipped");

        let renamed = rename_agent(&created.definition, "critic").expect("rename succeeds");
        assert_eq!(renamed.definition.local_name, "critic");
        assert_eq!(renamed.definition.name, "critic");
        assert!(!created.file_path.exists());
        assert!(renamed.file_path.exists());
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-014: read-only-source rejection for Builtin/Package targets (chains)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn create_chain_rejects_builtin_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = create_chain(tmp.path(), AgentSource::Builtin, "release", "desc");
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
    }

    #[test]
    fn create_chain_rejects_package_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = create_chain(tmp.path(), AgentSource::Package, "release", "desc");
        assert!(matches!(result, Err(SubagentError::ReadOnlySource(_))));
    }

    #[test]
    fn create_chain_succeeds_for_user_and_project_sources() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_chain = create_chain(tmp.path(), AgentSource::User, "release", "desc")
            .expect("user chain create succeeds");
        assert_eq!(user_chain.source, AgentSource::User);

        let tmp2 = tempfile::tempdir().expect("tempdir");
        let project_chain = create_chain(tmp2.path(), AgentSource::Project, "release", "desc")
            .expect("project chain create succeeds");
        assert_eq!(project_chain.source, AgentSource::Project);
    }

    #[test]
    fn update_chain_rejects_builtin_and_package_sources() {
        let builtin = sample_chain(AgentSource::Builtin, PathBuf::from("/builtin/release.chain.json"));
        assert!(matches!(
            update_chain(&builtin, &ChainFields::default()),
            Err(SubagentError::ReadOnlySource(_))
        ));

        let package = sample_chain(AgentSource::Package, PathBuf::from("/pkg/release.chain.json"));
        assert!(matches!(
            update_chain(&package, &ChainFields::default()),
            Err(SubagentError::ReadOnlySource(_))
        ));
    }

    #[test]
    fn delete_chain_rejects_builtin_and_package_sources() {
        let builtin = sample_chain(AgentSource::Builtin, PathBuf::from("/builtin/release.chain.json"));
        assert!(matches!(
            delete_chain(&builtin),
            Err(SubagentError::ReadOnlySource(_))
        ));

        let package = sample_chain(AgentSource::Package, PathBuf::from("/pkg/release.chain.json"));
        assert!(matches!(
            delete_chain(&package),
            Err(SubagentError::ReadOnlySource(_))
        ));
    }

    #[test]
    fn rename_chain_rejects_builtin_and_package_sources() {
        let builtin = sample_chain(AgentSource::Builtin, PathBuf::from("/builtin/release.chain.json"));
        assert!(matches!(
            rename_chain(&builtin, "new-name"),
            Err(SubagentError::ReadOnlySource(_))
        ));

        let package = sample_chain(AgentSource::Package, PathBuf::from("/pkg/release.chain.json"));
        assert!(matches!(
            rename_chain(&package, "new-name"),
            Err(SubagentError::ReadOnlySource(_))
        ));
    }

    #[test]
    fn delete_and_rename_chain_succeed_for_project_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let created = create_chain(tmp.path(), AgentSource::Project, "release", "desc")
            .expect("create succeeds");

        let renamed = rename_chain(&created, "ship-it").expect("rename succeeds");
        assert_eq!(renamed.name, "ship-it");
        assert!(!created.file_path.exists());
        assert!(renamed.file_path.exists());

        delete_chain(&renamed).expect("delete succeeds");
        assert!(!renamed.file_path.exists());
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-013: three-way disabled-visibility split
    // -----------------------------------------------------------------------------------------

    fn agent_named(name: &str, disabled: Option<bool>) -> AgentDefinition {
        let mut a = sample_agent(AgentSource::Project, PathBuf::from(format!("/proj/{name}.md")));
        a.name = name.to_string();
        a.local_name = name.to_string();
        a.disabled = disabled;
        a
    }

    #[test]
    fn management_view_includes_disabled_agents() {
        let agents = vec![
            agent_named("enabled-one", None),
            agent_named("disabled-one", Some(true)),
            agent_named("explicitly-enabled", Some(false)),
        ];
        let visible = AgentVisibility::management(&agents);
        assert_eq!(visible.len(), 3, "management view MUST include disabled agents");
        assert!(visible.iter().any(|a| a.name == "disabled-one"));
    }

    #[test]
    fn delegation_view_excludes_disabled_agents() {
        let agents = vec![
            agent_named("enabled-one", None),
            agent_named("disabled-one", Some(true)),
            agent_named("explicitly-enabled", Some(false)),
        ];
        let visible = AgentVisibility::delegation(&agents);
        assert_eq!(visible.len(), 2, "delegation view MUST exclude disabled agents");
        assert!(!visible.iter().any(|a| a.name == "disabled-one"));
        assert!(visible.iter().any(|a| a.name == "enabled-one"));
        assert!(visible.iter().any(|a| a.name == "explicitly-enabled"));
    }

    #[test]
    fn list_view_excludes_disabled_agents_independently_of_delegation() {
        let agents = vec![
            agent_named("enabled-one", None),
            agent_named("disabled-one", Some(true)),
        ];
        let list_visible = AgentVisibility::list(&agents);
        let delegation_visible = AgentVisibility::delegation(&agents);
        // Same current predicate, but the two are asserted independently (distinct function
        // calls, distinct assertions) so a future divergence between them is caught by whichever
        // assertion regresses, not silently passed by a single shared check.
        assert_eq!(list_visible.len(), 1);
        assert_eq!(delegation_visible.len(), 1);
        assert!(!list_visible.iter().any(|a| a.name == "disabled-one"));
        assert!(!delegation_visible.iter().any(|a| a.name == "disabled-one"));
    }

    #[test]
    fn three_visibility_views_diverge_exactly_on_disabled_agents() {
        let agents = vec![
            agent_named("a", None),
            agent_named("b", Some(true)),
            agent_named("c", Some(false)),
        ];
        assert_eq!(AgentVisibility::management(&agents).len(), 3);
        assert_eq!(AgentVisibility::delegation(&agents).len(), 2);
        assert_eq!(AgentVisibility::list(&agents).len(), 2);
    }

    #[test]
    fn chain_visibility_views_are_all_unfiltered_passthroughs() {
        let chains = vec![
            sample_chain(AgentSource::User, PathBuf::from("/user/a.chain.json")),
            sample_chain(AgentSource::Project, PathBuf::from("/proj/a.chain.json")),
        ];
        assert_eq!(ChainVisibility::management(&chains).len(), 2);
        assert_eq!(ChainVisibility::delegation(&chains).len(), 2);
        assert_eq!(ChainVisibility::list(&chains).len(), 2);
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-004/011 taxonomy: invalid package identifier -> silent skip, not an error
    // -----------------------------------------------------------------------------------------

    #[test]
    fn create_agent_with_invalid_package_identifier_is_silently_skipped_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let fields = AgentFields {
            package_name: Some(Some("!!!".to_string())),
            ..AgentFields::default()
        };

        let result = create_agent(tmp.path(), AgentSource::User, "scout", "desc", &fields)
            .expect("must be Ok, not Err, per R-SA-006 silent-skip taxonomy");
        assert!(
            result.is_none(),
            "invalid package identifier must produce Ok(None), not a written file"
        );
        assert!(
            !tmp.path().join("scout.md").exists(),
            "no file should be written when the package identifier is invalid"
        );
    }

    #[test]
    fn update_agent_with_invalid_package_identifier_is_silently_skipped_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let created = create_agent(
            tmp.path(),
            AgentSource::User,
            "scout",
            "desc",
            &AgentFields::default(),
        )
        .expect("no error")
        .expect("not skipped");

        // "!!!" contains no `[a-z0-9.-]` characters at all, so it normalizes to the empty
        // string and fails validation — unlike e.g. "Not Valid!!!", which would normalize to
        // the *valid* identifier "not-valid" (whitespace becomes "-", "!!!" is stripped).
        let fields = AgentFields {
            package_name: Some(Some("!!!".to_string())),
            ..AgentFields::default()
        };
        let result = update_agent(&created.definition, &fields)
            .expect("must be Ok, not Err, per R-SA-006 silent-skip taxonomy");
        assert!(result.is_none(), "invalid package identifier must produce Ok(None)");

        // Original file is untouched by the skipped update.
        let content = std::fs::read_to_string(&created.file_path).expect("read back");
        assert!(!content.contains("package:"));
    }

    #[test]
    fn create_agent_with_valid_package_identifier_normalizes_and_succeeds() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let fields = AgentFields {
            package_name: Some(Some("Code Analysis!".to_string())),
            ..AgentFields::default()
        };

        let outcome = create_agent(tmp.path(), AgentSource::User, "scout", "desc", &fields)
            .expect("no error")
            .expect("valid-after-normalization package identifier must not be skipped");
        assert_eq!(outcome.definition.package_name, Some("code-analysis".to_string()));
        assert_eq!(outcome.definition.name, "code-analysis.scout");
    }

    #[test]
    fn create_agent_with_absent_package_field_is_not_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outcome = create_agent(
            tmp.path(),
            AgentSource::User,
            "scout",
            "desc",
            &AgentFields::default(),
        )
        .expect("no error")
        .expect("absent package field must not trigger a skip");
        assert_eq!(outcome.definition.package_name, None);
    }

    #[test]
    fn normalize_package_identifier_matches_frontmatter_rs_validation_fixtures() {
        // Same fixture set `frontmatter.rs`'s own tests pin, to guard the two duplicated
        // implementations against drift (see this module's header note on why the validator is
        // duplicated rather than imported).
        assert_eq!(normalize_package_identifier(None), None);
        assert_eq!(normalize_package_identifier(Some("")), None);
        assert_eq!(normalize_package_identifier(Some("   ---   ")), None);
        assert_eq!(normalize_package_identifier(Some("!!!")), None);
        assert_eq!(
            normalize_package_identifier(Some("Code Analysis!")),
            Some("code-analysis".to_string())
        );
        assert_eq!(
            normalize_package_identifier(Some("acme")),
            Some("acme".to_string())
        );
        assert_eq!(
            normalize_package_identifier(Some("acme.tools")),
            Some("acme.tools".to_string())
        );
    }

    // -----------------------------------------------------------------------------------------
    // Round-trip fidelity: write then re-parse yields an equivalent definition
    // -----------------------------------------------------------------------------------------

    #[test]
    fn created_agent_round_trips_through_discovery_parser() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let fields = AgentFields {
            tools: Some(Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Mcp("filesystem.list".to_string()),
            ])),
            model: Some(Some(ModelId::from("anthropic/claude-sonnet-4"))),
            thinking: Some(Some(ThinkingLevel::High)),
            system_prompt_body: Some("You investigate things.".to_string()),
            ..AgentFields::default()
        };

        let outcome = create_agent(tmp.path(), AgentSource::Project, "investigator", "Investigates", &fields)
            .expect("no error")
            .expect("not skipped");

        assert_eq!(
            outcome.definition.tools,
            Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Mcp("filesystem.list".to_string()),
            ])
        );
        assert_eq!(outcome.definition.model, Some(ModelId::from("anthropic/claude-sonnet-4")));
        assert_eq!(outcome.definition.thinking, Some(ThinkingLevel::High));
        assert_eq!(outcome.definition.system_prompt_body, "You investigate things.");
    }

    #[test]
    fn create_agent_fails_when_file_already_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        create_agent(
            tmp.path(),
            AgentSource::User,
            "scout",
            "desc",
            &AgentFields::default(),
        )
        .expect("no error")
        .expect("not skipped");

        let second = create_agent(
            tmp.path(),
            AgentSource::User,
            "scout",
            "desc",
            &AgentFields::default(),
        );
        assert!(second.is_err(), "creating over an existing file must fail");
    }

    #[test]
    fn rename_agent_fails_when_destination_already_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = create_agent(tmp.path(), AgentSource::User, "a", "desc", &AgentFields::default())
            .expect("no error")
            .expect("not skipped");
        create_agent(tmp.path(), AgentSource::User, "b", "desc", &AgentFields::default())
            .expect("no error")
            .expect("not skipped");

        let result = rename_agent(&a.definition, "b");
        assert!(result.is_err(), "renaming onto an existing file must fail");
        // Original file must still exist since the rename was rejected.
        assert!(a.file_path.exists());
    }
}
