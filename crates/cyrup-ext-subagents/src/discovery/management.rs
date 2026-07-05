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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use cyrup_core::ModelId;

use crate::error::SubagentError;
use crate::fork_context::ContextMode;

use super::types::{
    AgentDefinition, AgentModelSourceInfo, AgentSource, ChainDefinition, ChainDiscoveryDiagnostic,
    ChainListBinding, ChainOutputBinding, ChainStepConfig, OutputSpec, OverrideScope,
    SystemPromptMode, ToolRef,
};
use super::{discover_agents_all, AgentDiscoveryConfig, AgentDiscoveryResult};

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
    /// Outer `Option` = "did the update config mention `thinking`"; inner `Option<String>` is pi's
    /// `string | false` — `Some(Some(s))` sets the OPEN reasoning string (`"off"`/`"high"`/arbitrary),
    /// `Some(None)` clears it (`false`/empty), `None` leaves the existing value untouched.
    pub thinking: Option<Option<String>>,
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

    // CREATE emits the default field set (no preserve semantics — there is no prior on-disk file).
    write_agent_file(&file_path, &definition, None)?;
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

    // UPDATE preserves the file's existing frontmatter field set (minus the keys this call changes,
    // plus pi's re-pinned keys) so an update never silently drops a present-but-default-valued or
    // unknown key. `existing.present_fields` is the on-disk field set; the update handler already
    // fed us the pre-override `editable_base`, so no settings-override value is baked into the file.
    let preserve = preserved_frontmatter_fields(&existing.present_fields, fields);
    write_agent_file(&existing.file_path, &merged, Some(&preserve))?;
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

    // RENAME re-serializes the (content-unchanged) definition under the new path. Passing `None`
    // keeps the pre-existing create-style emission rules — rename is not a field-editing update, so
    // pi's preserve-frontmatter-fields update semantics do not apply here.
    write_agent_file(&new_path, &renamed, None)?;
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
        thinking: fields.thinking.clone().unwrap_or(None),
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
        thinking: fields
            .thinking
            .clone()
            .unwrap_or_else(|| existing.thinking.clone()),
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
// Frontmatter serialization (write-back) — a faithful port of pi's
// `serializeAgent(config, { preserveFrontmatterFields })` (`agent-serializer.ts:37-110`).
//
// Two round-trip properties this writer upholds (T7 §3):
//   * PRESERVE-FRONTMATTER-FIELDS on UPDATE: when `preserve_fields` is `Some`, a key that was
//     present on disk but is NOT being changed by this update is re-emitted even if it would
//     otherwise be omitted, so an update never silently drops the file's existing field set. On
//     CREATE (`None`) the default field set is emitted (systemPromptMode/inheritProjectContext/
//     inheritSkills always present).
//   * BLOCK-VALUED EXTRA FIELDS survive rewrite: an `extra_fields` value with embedded newlines
//     (e.g. a `permission:` nested-YAML block captured by `frontmatter.rs`) is re-emitted as
//     `key:` + two-space-indented lines, NOT corrupted into one flat line.
// Settings-override values are NOT baked into files: the update handler feeds `serialize_agent` the
// pre-override `editable_base` snapshot (pi `editableAgentConfig`, `agent-management.ts:174-196`),
// and `disabled` is never emitted at all (it is a settings-only concept — a `disabled:` in an agent
// file is just an unknown extra field, round-tripped through `extra_fields`).
// -------------------------------------------------------------------------------------------

fn write_agent_file(
    file_path: &Path,
    definition: &AgentDefinition,
    preserve_fields: Option<&HashSet<String>>,
) -> Result<(), SubagentError> {
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(SubagentError::Spawn)?;
    }
    let content = serialize_agent(definition, preserve_fields);
    std::fs::write(file_path, content).map_err(SubagentError::Spawn)?;
    Ok(())
}

fn serialize_agent(def: &AgentDefinition, preserve_fields: Option<&HashSet<String>>) -> String {
    // `preserve(&[..])` mirrors pi's `preserve(...fields)` (true iff any listed key is in the set);
    // `preserving_existing` mirrors `preservingExistingFrontmatter` (`Some` == an UPDATE that must
    // round-trip the file's existing field set; `None` == a CREATE emitting the default field set).
    let preserve = |fields: &[&str]| -> bool {
        preserve_fields.is_some_and(|set| fields.iter().any(|f| set.contains(*f)))
    };
    let preserving_existing = preserve_fields.is_some();

    let mut lines: Vec<String> = Vec::new();
    lines.push("---".to_string());
    lines.push(format!("name: {}", def.local_name));
    if let Some(pkg) = &def.package_name {
        lines.push(format!("package: {pkg}"));
    }
    lines.push(format!("description: {}", def.description));

    // tools: agent builtin/extension entries plus `mcp:`-prefixed direct tools (pi merges both).
    let tools_value = def.tools.as_ref().and_then(|tools| {
        if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(tool_ref_to_frontmatter_entry)
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        }
    });
    if tools_value.is_some() || preserve(&["tools"]) {
        lines.push(format!("tools: {}", tools_value.as_deref().unwrap_or("")));
    }

    if def.model.is_some() || preserve(&["model"]) {
        let model_str = def.model.as_ref().map(ToString::to_string).unwrap_or_default();
        lines.push(format!("model: {model_str}"));
    }

    let fallback_value = if def.fallback_models.is_empty() {
        None
    } else {
        Some(
            def.fallback_models
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        )
    };
    if fallback_value.is_some() || preserve(&["fallbackModels"]) {
        lines.push(format!(
            "fallbackModels: {}",
            fallback_value.as_deref().unwrap_or("")
        ));
    }

    // thinking is an OPEN string. Emit the raw value unless it is unset (only under preserve) or
    // exactly `off` (only under preserve — a fresh serialize does not bake a redundant `thinking:
    // off`), matching `agent-serializer.ts:56-58`.
    {
        let thinking = def.thinking.as_deref();
        let has_thinking = thinking.is_some_and(|t| !t.is_empty());
        let is_off = thinking == Some("off");
        let emit = (has_thinking && (!is_off || preserve(&["thinking"])))
            || (!has_thinking && preserve(&["thinking"]));
        if emit {
            lines.push(format!("thinking: {}", thinking.unwrap_or("")));
        }
    }

    if !preserving_existing || preserve(&["systemPromptMode"]) {
        lines.push(format!(
            "systemPromptMode: {}",
            match def.system_prompt_mode {
                SystemPromptMode::Append => "append",
                SystemPromptMode::Replace => "replace",
            }
        ));
    }
    if !preserving_existing || preserve(&["inheritProjectContext"]) {
        lines.push(format!(
            "inheritProjectContext: {}",
            def.inherit_project_context
        ));
    }
    if !preserving_existing || preserve(&["inheritSkills"]) {
        lines.push(format!("inheritSkills: {}", def.inherit_skills));
    }

    if def.default_context.is_some() || preserve(&["defaultContext"]) {
        lines.push(format!(
            "defaultContext: {}",
            match def.default_context {
                Some(ContextMode::Fork) => "fork",
                Some(ContextMode::Fresh) => "fresh",
                None => "",
            }
        ));
    }

    let skills_value = if def.skills.is_empty() {
        None
    } else {
        Some(def.skills.join(", "))
    };
    if skills_value.is_some() || preserve(&["skill", "skills"]) {
        lines.push(format!("skills: {}", skills_value.as_deref().unwrap_or("")));
    }

    if let Some(exts) = &def.extensions {
        lines.push(format!("extensions: {}", exts.join(", ")));
    }
    if !def.subagent_only_extensions.is_empty() || preserve(&["subagentOnlyExtensions"]) {
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
    if let Some(reads) = &def.default_reads
        && !reads.is_empty()
    {
        let joined = reads
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("defaultReads: {joined}"));
    }
    if def.default_progress == Some(true) {
        lines.push("defaultProgress: true".to_string());
    }
    if def.interactive == Some(true) {
        lines.push("interactive: true".to_string());
    }
    if let Some(depth) = def.max_subagent_depth {
        lines.push(format!("maxSubagentDepth: {depth}"));
    }
    // completionGuard: pi emits it only when explicitly disabled (`=== false`) or under preserve,
    // writing "" for an undefined value under preserve (`agent-serializer.ts:87-89`).
    if def.completion_guard == Some(false) || preserve(&["completionGuard"]) {
        let value = match def.completion_guard {
            None => "",
            Some(true) => "true",
            Some(false) => "false",
        };
        lines.push(format!("completionGuard: {value}"));
    }

    // Unknown-key round-trip (`agent-serializer.ts:91-104`): re-emit every extra field, skipping any
    // key that is actually a KNOWN field (defensive; the parser never puts one here). A block value
    // (embedded newlines) is re-emitted as `key:` + each line indented two spaces so it round-trips
    // back through `parse_frontmatter_block`'s block grammar instead of being corrupted into a flat
    // line; a flat value is emitted as `key: value`.
    for (key, value) in &def.extra_fields {
        if crate::discovery::frontmatter::is_known_field(key) {
            continue;
        }
        if value.contains('\n') {
            lines.push(format!("{key}:"));
            for block_line in value.split('\n') {
                lines.push(format!("  {block_line}"));
            }
        } else {
            lines.push(format!("{key}: {value}"));
        }
    }

    lines.push("---".to_string());
    format!("{}\n\n{}\n", lines.join("\n"), def.system_prompt_body)
}

/// pi `preservedAgentFrontmatterFields` (`agent-management.ts:207-250`): starting from the field
/// keys literally present in the agent's on-disk frontmatter (`existing_present`, which for a
/// discovered agent is exactly [`AgentDefinition::present_fields`]), REMOVE any key this update is
/// changing (so the changed field is re-serialized from its NEW value, not preserved at its old
/// one), then ADD BACK the keys pi re-pins even when changed: `systemPromptMode`/
/// `inheritProjectContext`/`inheritSkills` always; `thinking` only when set to exactly `off`;
/// `completionGuard` only when set to `true`. The result is the `preserveFrontmatterFields` set
/// [`serialize_agent`] consults for a preserve-aware UPDATE.
///
/// `fields.<x>.is_some()` is the faithful analog of pi's `hasKey(cfg, "<x>")` because the update
/// handler's `apply_agent_config` sets each `AgentFields` slot ONLY when the config named that key —
/// except `local_name`/`package_name`/`description`, which the handler always sets; those three keys
/// are never gated by the preserve set in `serialize_agent` (name/description are emitted
/// unconditionally, package only when present), so eagerly removing them here is a harmless no-op.
fn preserved_frontmatter_fields(
    existing_present: &HashSet<String>,
    fields: &AgentFields,
) -> HashSet<String> {
    let mut set: HashSet<String> = existing_present.clone();
    if fields.local_name.is_some() {
        set.remove("name");
    }
    if fields.package_name.is_some() {
        set.remove("package");
    }
    if fields.description.is_some() {
        set.remove("description");
    }
    if fields.system_prompt_body.is_some() {
        set.remove("systemPrompt");
    }
    if fields.model.is_some() {
        set.remove("model");
    }
    if fields.fallback_models.is_some() {
        set.remove("fallbackModels");
    }
    if fields.tools.is_some() {
        set.remove("tools");
    }
    if fields.skills.is_some() {
        set.remove("skill");
        set.remove("skills");
    }
    if fields.extensions.is_some() {
        set.remove("extensions");
    }
    if fields.subagent_only_extensions.is_some() {
        set.remove("subagentOnlyExtensions");
    }
    if fields.thinking.is_some() {
        set.remove("thinking");
        if matches!(fields.thinking.as_ref().and_then(|o| o.as_deref()), Some("off")) {
            set.insert("thinking".to_string());
        }
    }
    if fields.system_prompt_mode.is_some() {
        set.remove("systemPromptMode");
        set.insert("systemPromptMode".to_string());
    }
    if fields.inherit_project_context.is_some() {
        set.remove("inheritProjectContext");
        set.insert("inheritProjectContext".to_string());
    }
    if fields.inherit_skills.is_some() {
        set.remove("inheritSkills");
        set.insert("inheritSkills".to_string());
    }
    if fields.default_context.is_some() {
        set.remove("defaultContext");
    }
    if fields.output.is_some() {
        set.remove("output");
    }
    if fields.default_reads.is_some() {
        set.remove("defaultReads");
    }
    if fields.default_progress.is_some() {
        set.remove("defaultProgress");
    }
    if fields.max_subagent_depth.is_some() {
        set.remove("maxSubagentDepth");
    }
    if fields.completion_guard.is_some() {
        set.remove("completionGuard");
        if matches!(fields.completion_guard, Some(Some(true))) {
            set.insert("completionGuard".to_string());
        }
    }
    set
}

fn tool_ref_to_frontmatter_entry(tool: &ToolRef) -> String {
    match tool {
        ToolRef::Builtin(name) | ToolRef::ExtensionPath(name) => name.clone(),
        ToolRef::Mcp(name) => format!("mcp:{name}"),
    }
}

// -------------------------------------------------------------------------------------------
// Chain create/update/delete/rename (R-SA-014 applies identically to chains)
// -------------------------------------------------------------------------------------------

/// The caller-supplied delta for a chain create/update — deliberately minimal. This module's own
/// job (R-SA-013/014/019 CRUD + visibility) does not include full chain-step authoring (building
/// up a real [`ChainStepConfig`] sequence is a chain-editor concern, not a bare-CRUD concern) —
/// `step_count` only controls how many placeholder [`ChainStepConfig`] entries this module
/// materializes when it needs to preserve or resize a chain's step list without inventing per-step
/// content of its own. A future chain-editor-facing API (outside this file's R-SA-013/014/019
/// scope) would supply real [`ChainStepConfig`] values directly rather than going through
/// `step_count`.
#[derive(Clone, Debug, Default)]
pub struct ChainFields {
    pub name: Option<String>,
    pub description: Option<String>,
    pub step_count: Option<usize>,
}

/// Build one minimal, empty placeholder [`ChainStepConfig`] — used only to preserve step *count*
/// across a management-layer chain update that does not itself author step content (see
/// [`ChainFields`]'s own doc). Every field left at its default (`None`/empty) so this placeholder
/// carries no spurious behavior if ever (mis)dispatched directly.
fn placeholder_chain_step() -> ChainStepConfig {
    ChainStepConfig::default()
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
        local_name: name.to_string(),
        package_name: None,
        description: description.to_string(),
        source,
        file_path: file_path.clone(),
        steps: Vec::new(),
        extra_fields: BTreeMap::new(),
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
        name: name.clone(),
        local_name: name,
        package_name: existing.package_name.clone(),
        description,
        source: existing.source,
        file_path: existing.file_path.clone(),
        steps: (0..step_count).map(|_| placeholder_chain_step()).collect(),
        extra_fields: existing.extra_fields.clone(),
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
        local_name: new_name.to_string(),
        package_name: existing.package_name.clone(),
        description: existing.description.clone(),
        source: existing.source,
        file_path: new_path.clone(),
        steps: existing.steps.clone(),
        extra_fields: existing.extra_fields.clone(),
    };

    write_chain_file(&new_path, &renamed)?;
    std::fs::remove_file(&existing.file_path).map_err(SubagentError::Spawn)?;
    Ok(renamed)
}

/// Create a new `.chain.json` under `scope_dir` carrying real authored `steps` and an optional
/// package identifier — the steps-aware create the management `action: "create"` path needs (pi
/// `handleCreate`'s chain branch, `agent-management.ts:706-715`). Unlike the bare [`create_chain`]
/// skeleton (which materializes an empty step list), this preserves the caller's parsed
/// [`ChainStepConfig`] sequence verbatim. The file is named by the RUNTIME name
/// (`{package}.{local}.chain.json`), matching pi's on-disk convention, and always written as
/// `.chain.json` (this module's own sanctioned chain-format convention).
pub fn create_chain_with_steps(
    scope_dir: &Path,
    source: AgentSource,
    local_name: &str,
    package_name: Option<String>,
    description: &str,
    steps: Vec<ChainStepConfig>,
) -> Result<ChainDefinition, SubagentError> {
    require_writable_source(source, local_name)?;
    let runtime_name = AgentDefinition::qualified_name(local_name, package_name.as_deref());
    let file_path = chain_file_path(scope_dir, &runtime_name);
    if file_path.exists() {
        return Err(SubagentError::Spawn(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("chain file already exists: {}", file_path.display()),
        )));
    }
    let definition = ChainDefinition {
        name: runtime_name,
        local_name: local_name.to_string(),
        package_name,
        description: description.to_string(),
        source,
        file_path: file_path.clone(),
        steps,
        extra_fields: BTreeMap::new(),
    };
    write_chain_file(&file_path, &definition)?;
    Ok(definition)
}

/// Update an existing User/Project-scope chain in one step: rewrite `name`/`package`/`description`/
/// `steps` and rename the on-disk file when the runtime name changes — the steps-preserving update
/// the management `action: "update"` path needs (pi `handleUpdate`'s chain branch,
/// `agent-management.ts:802-846`). Unlike the bare [`update_chain`] skeleton (which replaces the
/// step list with empty placeholders), this writes the caller-supplied `steps` verbatim (the
/// handler passes the existing steps through when the caller did not re-author them). Fails with
/// `ReadOnlySource` for a Builtin/Package target (R-SA-014) before any filesystem access, and with
/// an `AlreadyExists`-kind error if a rename would collide.
pub fn update_chain_full(
    existing: &ChainDefinition,
    new_local_name: &str,
    package_name: Option<String>,
    description: &str,
    steps: Vec<ChainStepConfig>,
) -> Result<ChainDefinition, SubagentError> {
    require_writable_source(existing.source, &existing.name)?;
    let runtime_name = AgentDefinition::qualified_name(new_local_name, package_name.as_deref());
    let scope_dir = existing
        .file_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let new_path = chain_file_path(&scope_dir, &runtime_name);
    if new_path != existing.file_path && new_path.exists() {
        return Err(SubagentError::Spawn(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("chain file already exists: {}", new_path.display()),
        )));
    }
    let updated = ChainDefinition {
        name: runtime_name,
        local_name: new_local_name.to_string(),
        package_name,
        description: description.to_string(),
        source: existing.source,
        file_path: new_path.clone(),
        steps,
        extra_fields: existing.extra_fields.clone(),
    };
    write_chain_file(&new_path, &updated)?;
    if new_path != existing.file_path {
        std::fs::remove_file(&existing.file_path).map_err(SubagentError::Spawn)?;
    }
    Ok(updated)
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

/// Serialize a [`ChainDefinition`] to the pi `.chain.json` shape
/// [`crate::discovery::chains::parse_chain_json`] reads (`serializeJsonChain`,
/// `chain-serializer.ts:201-214`): a root object with the pre-qualification `name`
/// (`frontmatterNameForConfig` — the chain's `local_name`), `description`, and a `chain` ARRAY of
/// the raw [`ChainStepConfig`] steps, plus `package` when set and any preserved `extra_fields`.
fn serialize_chain_json(def: &ChainDefinition) -> String {
    let chain: Vec<serde_json::Value> = def
        .steps
        .iter()
        .map(|step| serde_json::to_value(step).unwrap_or_else(|_| serde_json::json!({})))
        .collect();

    let mut root = serde_json::Map::new();
    root.insert(
        "name".to_string(),
        serde_json::Value::String(def.local_name.clone()),
    );
    root.insert(
        "description".to_string(),
        serde_json::Value::String(def.description.clone()),
    );
    root.insert("chain".to_string(), serde_json::Value::Array(chain));
    if let Some(package) = &def.package_name {
        root.insert(
            "package".to_string(),
            serde_json::Value::String(package.clone()),
        );
    }
    for (key, value) in &def.extra_fields {
        if key != "name" && key != "description" && key != "package" && key != "chain" {
            root.insert(key.clone(), serde_json::Value::String(value.clone()));
        }
    }

    serde_json::to_string_pretty(&serde_json::Value::Object(root))
        .unwrap_or_else(|_| "{}".to_string())
}

// ===============================================================================================
// Management-action dispatch + renderers — a faithful port of pi's `agent-management.ts`
// `handleManagementAction`/`handleList`/`handleGet`/`handleModels`/`handleCreate`/`handleUpdate`/
// `handleDelete` + `formatAgentDetail`/`formatChainDetail`/`formatChainStepDetail`
// (`agent-management.ts:44-880`). This is the C3 wire-up: the low-level CRUD above finally gains its
// real production callers, driven by the `subagent` tool's `action` enum
// (`list`/`get`/`models`/`create`/`update`/`delete`) via `extension.rs::route_management_action`.
//
// Deferred here (documented, NOT silently claimed done), each gated on a subsystem outside this C3
// task's scope:
//   - model-registry warnings (`modelWarning`/`fallbackModelsWarning`) and live model resolution in
//     `models` — need the model-registry / session-model handle that is `outer-layer` (Tier 8) and
//     not threaded into this crate; the mapping here degrades to the agent's own discovery-resolved
//     `model` + override provenance without a live registry (see [`format_model_source`]).
//   - skill warnings (`skillsWarning`) and proactive-skill suggestions in `list` — need the skills
//     subsystem (C4 / Tier 5), entirely absent today.
//   - companion suggestions in `list` — deferred-companion (no cyrup companion exists to integrate).
//   - settings-override un-apply on update (`editableAgentConfig`) — settings overrides are inert
//     today (C2 / Tier 2), so `override_info` is always `None` and the un-apply ([`editable_base`])
//     is a no-op; it is still applied forward-compatibly here so the moment C2 lands it is correct.
//
// One architectural divergence (documented, NOT a management-layer bug): pi's `discoverAgentsAll`
// returns UNMERGED per-tier arrays (`agents.ts:1325-1422`), so `list`/`get` on a name that a
// user/project agent shadows across tiers show BOTH the builtin/package entry AND the shadowing
// entry. cyrup's `discover_agents_all` returns the R-SA-001 four-tier MERGE (one precedence-winner
// per name, by deliberate architecture), so `list`/`get` show only the winner. `update`/`delete`
// outcomes are UNAFFECTED — resolveTarget's mutable winner is identical either way. Reproducing pi's
// raw-tier duplicate view would require a separate unmerged discovery entry point (Tier-7 discovery
// scope), out of this C3 task. In the common (non-shadowing) case the output is byte-identical.
// ===============================================================================================

/// pi's `BUILTIN_AGENT_NAMES` (`agents.ts:25-34`) — used by [`handle_models`] to bound the requested
/// filter and to iterate the builtin model mapping in pi's exact stable order.
pub const BUILTIN_AGENT_NAMES: [&str; 8] = [
    "context-builder",
    "delegate",
    "oracle",
    "planner",
    "researcher",
    "reviewer",
    "scout",
    "worker",
];

/// The management-relevant subset of the `subagent` tool's parsed parameters (pi `ManagementParams`,
/// `agent-management.ts:36-42`). Borrowed from the caller's already-parsed `SubagentToolParams` so
/// `extension.rs` owns the JSON deserialization and this module owns only the management semantics.
pub struct ManagementRequest<'a> {
    pub agent: Option<&'a str>,
    pub chain_name: Option<&'a str>,
    pub agent_scope: Option<&'a str>,
    pub config: Option<&'a serde_json::Value>,
}

/// The rendered outcome of a management action — pi's `result(text, isError)`
/// (`agent-management.ts:44-46`). `is_error` mirrors pi's `AgentToolResult.isError`; the caller maps
/// `is_error == true` to a `ToolError` (cyrup surfaces tool failures as `Err`, R-02-024) while still
/// preserving pi's exact human-facing text verbatim.
pub struct ManagementOutcome {
    pub text: String,
    pub is_error: bool,
}

impl ManagementOutcome {
    fn ok(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: false }
    }
    fn err(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true }
    }
}

/// pi's `handleManagementAction` (`agent-management.ts:870-880`): dispatch a management `action` to
/// its handler. Discovery is re-run per call inside each handler (R-SA-019), never cached across a
/// create -> get -> update -> delete sequence.
///
/// # Errors
///
/// Propagates a discovery-time [`SubagentError`] (R-SA-009's malformed-settings abort) or a genuine
/// filesystem failure from a create/update/delete write. pi's `isError: true` outcomes (not-found,
/// read-only, validation) are `Ok(ManagementOutcome { is_error: true, .. })`, not `Err`.
pub fn handle_management_action(
    cfg: &AgentDiscoveryConfig,
    action: &str,
    req: &ManagementRequest,
) -> Result<ManagementOutcome, SubagentError> {
    match action {
        "list" => handle_list(cfg, req),
        "get" => handle_get(cfg, req),
        "models" => handle_models(cfg, req),
        "create" => handle_create(cfg, req),
        "update" => handle_update(cfg, req),
        "delete" => handle_delete(cfg, req),
        other => Ok(ManagementOutcome::err(format!("Unknown action: {other}"))),
    }
}

// -------------------------------------------------------------------------------------------
// Small shared helpers (source/context rendering, scope parsing, name sanitization, CSV/tools)
// -------------------------------------------------------------------------------------------

/// The camelCase source label pi renders (`AgentSource` serde `rename_all = "camelCase"`).
fn source_str(source: AgentSource) -> &'static str {
    match source {
        AgentSource::Builtin => "builtin",
        AgentSource::Package => "package",
        AgentSource::User => "user",
        AgentSource::Project => "project",
    }
}

fn context_str(mode: ContextMode) -> &'static str {
    match mode {
        ContextMode::Fresh => "fresh",
        ContextMode::Fork => "fork",
    }
}

fn override_scope_str(scope: OverrideScope) -> &'static str {
    match scope {
        OverrideScope::User => "user",
        OverrideScope::Project => "project",
    }
}

/// pi `asDisambiguationScope` (`agent-management.ts:70-73`): `"user"`/`"project"` pass through,
/// anything else (incl. absent / `"both"`) is `None`.
fn disambiguation_scope(scope: Option<&str>) -> Option<AgentSource> {
    match scope {
        Some("user") => Some(AgentSource::User),
        Some("project") => Some(AgentSource::Project),
        _ => None,
    }
}

/// pi `normalizeListScope` (`agent-management.ts:75-79`): absent -> both; `"user"`/`"project"`/
/// `"both"` pass through; any other value falls back to both. `None` here means "both".
fn normalize_list_scope(scope: Option<&str>) -> Option<AgentSource> {
    match scope {
        Some("user") => Some(AgentSource::User),
        Some("project") => Some(AgentSource::Project),
        _ => None,
    }
}

/// pi `sanitizeName` (`agent-management.ts:81-83`): `lowercase`, `trim`, `\s+`->`-`, strip
/// `[^a-z0-9-]`, `-+`->`-`, trim leading/trailing `-`.
fn sanitize_name(name: &str) -> String {
    let lowered = name.to_lowercase();
    let trimmed = lowered.trim();
    let mut ws_collapsed = String::with_capacity(trimmed.len());
    let mut last_ws = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !last_ws {
                ws_collapsed.push('-');
            }
            last_ws = true;
        } else {
            ws_collapsed.push(ch);
            last_ws = false;
        }
    }
    let filtered: String = ws_collapsed
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    let collapsed = collapse_repeated_char(&filtered, '-');
    collapsed.trim_matches('-').to_string()
}

/// pi `parseCsv` (`agent-management.ts:48-50`): split on `,`, trim, drop empties, dedup preserving
/// first occurrence.
fn parse_csv(value: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for part in value.split(',') {
        let trimmed = part.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// pi `parseTools` (`agent-management.ts:310-320`): split CSV; `mcp:`-prefixed entries become MCP
/// direct-tool refs (prefix stripped, verbatim otherwise), the rest builtin refs. cyrup unifies both
/// into one `Vec<ToolRef>` (MCP entries preserved as [`ToolRef::Mcp`] without the `mcp:` prefix,
/// matching [`tool_ref_to_frontmatter_entry`]'s inverse).
fn parse_tools(value: &str) -> Vec<ToolRef> {
    let mut out = Vec::new();
    for item in parse_csv(value) {
        if let Some(rest) = item.strip_prefix("mcp:") {
            let direct = rest.trim();
            if !direct.is_empty() {
                out.push(ToolRef::Mcp(direct.to_string()));
            }
        } else {
            out.push(ToolRef::Builtin(item));
        }
    }
    out
}


/// pi's name-sensitive create defaults (`agents.ts:36-45`): `delegate` -> `Append`/inherit-context,
/// else `Replace`/no-inherit; `inheritSkills` always defaults false. Replicated locally (matching
/// this crate's established "each module keeps its own small helper" convention) rather than making
/// `frontmatter.rs`'s private equivalents `pub(crate)`.
fn default_system_prompt_mode(local_name: &str) -> SystemPromptMode {
    if local_name == "delegate" {
        SystemPromptMode::Append
    } else {
        SystemPromptMode::Replace
    }
}

fn default_inherit_project_context(local_name: &str) -> bool {
    local_name == "delegate"
}

// -------------------------------------------------------------------------------------------
// config-object / package-name parsing (pi configObject / parsePackageName)
// -------------------------------------------------------------------------------------------

/// pi `configObject` (`agent-management.ts:52-64`): a JSON-string config is `JSON.parse`d (parse
/// failure -> `config must be valid JSON: …`); a non-object (or array) yields `Ok(None)`; an object
/// yields `Ok(Some(map))`.
fn config_object(
    config: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
    let Some(value) = config else {
        return Ok(None);
    };
    let owned;
    let val: &serde_json::Value = if let serde_json::Value::String(s) = value {
        owned = serde_json::from_str::<serde_json::Value>(s)
            .map_err(|e| format!("config must be valid JSON: {e}"))?;
        &owned
    } else {
        value
    };
    match val {
        serde_json::Value::Object(map) => Ok(Some(map.clone())),
        _ => Ok(None),
    }
}

/// pi `parsePackageName(value, "config.package")` (`identity.ts:11-17`): absent/`false`/`""` ->
/// `Ok(None)`; a non-string -> `Err(must be a string or false)`; a string that fails to normalize to
/// a valid identifier -> `Err(is invalid after sanitization)`. Note this is a HARD error at the
/// management layer, unlike the low-level [`create_agent`]/[`update_agent`] silent-skip (which this
/// handler never reaches, since it pre-validates here).
fn parse_package_config(value: Option<&serde_json::Value>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(serde_json::Value::Bool(false)) => Ok(None),
        Some(serde_json::Value::String(s)) if s.is_empty() => Ok(None),
        Some(serde_json::Value::String(s)) => match normalize_package_identifier(Some(s)) {
            Some(pkg) => Ok(Some(pkg)),
            None => Err("config.package is invalid after sanitization.".to_string()),
        },
        Some(_) => Err("config.package must be a string or false when provided.".to_string()),
    }
}

// -------------------------------------------------------------------------------------------
// applyAgentConfig: parse the caller's `config` object into an `AgentFields` delta (pi
// applyAgentConfig, agent-management.ts:322-417). Exact pi error strings are reproduced verbatim
// (the tool test-suite pins several, e.g. `config.completionGuard must be a boolean`).
// -------------------------------------------------------------------------------------------

fn apply_agent_config(
    fields: &mut AgentFields,
    cfg: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    use serde_json::Value;

    if let Some(v) = cfg.get("systemPrompt") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.system_prompt_body = Some(String::new());
        } else if let Some(s) = v.as_str() {
            fields.system_prompt_body = Some(s.to_string());
        } else {
            return Err("config.systemPrompt must be a string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("model") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.model = Some(None);
        } else if let Some(s) = v.as_str() {
            let trimmed = s.trim();
            fields.model = Some(if trimmed.is_empty() {
                None
            } else {
                Some(ModelId::from(trimmed))
            });
        } else {
            return Err("config.model must be a string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("fallbackModels") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.fallback_models = Some(Vec::new());
        } else if let Some(s) = v.as_str() {
            fields.fallback_models = Some(
                parse_csv(s)
                    .into_iter()
                    .map(|m| ModelId::from(m.as_str()))
                    .collect(),
            );
        } else if let Some(arr) = v.as_array() {
            let mut seen = HashSet::new();
            let mut models = Vec::new();
            for item in arr {
                if let Some(s) = item.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                        models.push(ModelId::from(trimmed));
                    }
                }
            }
            fields.fallback_models = Some(models);
        } else {
            return Err("config.fallbackModels must be a comma-separated string, string array, or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("tools") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.tools = Some(None);
        } else if let Some(s) = v.as_str() {
            let parsed = parse_tools(s);
            fields.tools = Some(if parsed.is_empty() { None } else { Some(parsed) });
        } else {
            return Err("config.tools must be a comma-separated string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("skills") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.skills = Some(Vec::new());
        } else if let Some(s) = v.as_str() {
            fields.skills = Some(parse_csv(s));
        } else {
            return Err("config.skills must be a comma-separated string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("extensions") {
        if v == &Value::Bool(false) {
            fields.extensions = Some(None);
        } else if v.as_str() == Some("") {
            fields.extensions = Some(Some(Vec::new()));
        } else if let Some(s) = v.as_str() {
            fields.extensions = Some(Some(parse_csv(s)));
        } else {
            return Err("config.extensions must be a comma-separated string, empty string, or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("subagentOnlyExtensions") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.subagent_only_extensions = Some(Vec::new());
        } else if let Some(s) = v.as_str() {
            fields.subagent_only_extensions = Some(parse_csv(s));
        } else {
            return Err("config.subagentOnlyExtensions must be a comma-separated string, empty string, or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("thinking") {
        // pi `applyAgentConfig` (`agent-management.ts:368-372`): `false`/`""` clears; any other
        // string sets the OPEN value (trimmed; a whitespace-only value clears). No closed-enum
        // coercion — an arbitrary/`off` value is preserved verbatim.
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.thinking = Some(None);
        } else if let Some(s) = v.as_str() {
            let trimmed = s.trim();
            fields.thinking = Some(if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            });
        } else {
            return Err("config.thinking must be a string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("systemPromptMode") {
        match v.as_str() {
            Some("append") => fields.system_prompt_mode = Some(SystemPromptMode::Append),
            Some("replace") => fields.system_prompt_mode = Some(SystemPromptMode::Replace),
            _ => {
                return Err("config.systemPromptMode must be 'append' or 'replace' when provided.".to_string())
            }
        }
    }
    if let Some(v) = cfg.get("inheritProjectContext") {
        match v.as_bool() {
            Some(b) => fields.inherit_project_context = Some(b),
            None => {
                return Err("config.inheritProjectContext must be a boolean when provided.".to_string())
            }
        }
    }
    if let Some(v) = cfg.get("inheritSkills") {
        match v.as_bool() {
            Some(b) => fields.inherit_skills = Some(b),
            None => return Err("config.inheritSkills must be a boolean when provided.".to_string()),
        }
    }
    if let Some(v) = cfg.get("defaultContext") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.default_context = Some(None);
        } else if v.as_str() == Some("fresh") {
            fields.default_context = Some(Some(ContextMode::Fresh));
        } else if v.as_str() == Some("fork") {
            fields.default_context = Some(Some(ContextMode::Fork));
        } else {
            return Err("config.defaultContext must be 'fresh', 'fork', or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("output") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.output = Some(None);
        } else if let Some(s) = v.as_str() {
            fields.output = Some(Some(OutputSpec {
                path: Some(PathBuf::from(s)),
                mode: None,
            }));
        } else {
            return Err("config.output must be a string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("reads") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.default_reads = Some(None);
        } else if let Some(s) = v.as_str() {
            let reads: Vec<PathBuf> = parse_csv(s).into_iter().map(PathBuf::from).collect();
            fields.default_reads = Some(if reads.is_empty() { None } else { Some(reads) });
        } else {
            return Err("config.reads must be a comma-separated string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("progress") {
        match v.as_bool() {
            Some(b) => fields.default_progress = Some(Some(b)),
            None => return Err("config.progress must be a boolean when provided.".to_string()),
        }
    }
    if let Some(v) = cfg.get("maxSubagentDepth") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.max_subagent_depth = Some(None);
        } else if let Some(n) = v.as_u64() {
            match u32::try_from(n) {
                Ok(depth) => fields.max_subagent_depth = Some(Some(depth)),
                Err(_) => {
                    return Err("config.maxSubagentDepth must be an integer >= 0 or false when provided.".to_string())
                }
            }
        } else {
            return Err("config.maxSubagentDepth must be an integer >= 0 or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("completionGuard") {
        match v.as_bool() {
            Some(b) => fields.completion_guard = Some(Some(b)),
            None => return Err("config.completionGuard must be a boolean when provided.".to_string()),
        }
    }
    Ok(())
}

// -------------------------------------------------------------------------------------------
// parseStepList: parse config.steps -> Vec<ChainStepConfig> (pi parseStepList, 252-308)
// -------------------------------------------------------------------------------------------

fn parse_step_list(raw: Option<&serde_json::Value>) -> Result<Vec<ChainStepConfig>, String> {
    use serde_json::Value;
    let Some(Value::Array(arr)) = raw else {
        return Err("config.steps must be an array.".to_string());
    };
    if arr.is_empty() {
        return Err("config.steps must include at least one step.".to_string());
    }
    let mut steps = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            return Err(format!("config.steps[{i}] must be an object."));
        };
        let agent = match obj.get("agent").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => return Err(format!("config.steps[{i}].agent must be a non-empty string.")),
        };
        let mut step = ChainStepConfig {
            agent: Some(agent),
            task: Some(obj.get("task").and_then(Value::as_str).unwrap_or("").to_string()),
            ..ChainStepConfig::default()
        };
        if let Some(v) = obj.get("phase") {
            match v.as_str() {
                Some(s) => step.phase = Some(s.to_string()),
                None => return Err(format!("config.steps[{i}].phase must be a string.")),
            }
        }
        if let Some(v) = obj.get("label") {
            match v.as_str() {
                Some(s) => step.label = Some(s.to_string()),
                None => return Err(format!("config.steps[{i}].label must be a string.")),
            }
        }
        if let Some(v) = obj.get("as") {
            match v.as_str() {
                Some(s) => step.as_ = Some(s.to_string()),
                None => return Err(format!("config.steps[{i}].as must be a string.")),
            }
        }
        if let Some(v) = obj.get("outputSchema") {
            match v.as_str() {
                Some(s) => step.output_schema = Some(Value::String(s.to_string())),
                None => {
                    return Err(format!("config.steps[{i}].outputSchema must be a schema file path string for saved chains."))
                }
            }
        }
        if let Some(v) = obj.get("output") {
            if v == &Value::Bool(false) {
                step.output = Some(ChainOutputBinding::Toggle(false));
            } else if let Some(s) = v.as_str() {
                step.output = Some(ChainOutputBinding::Name(s.to_string()));
            } else {
                return Err(format!("config.steps[{i}].output must be a string or false."));
            }
        }
        if let Some(v) = obj.get("outputMode") {
            match v.as_str() {
                Some("inline") => step.output_mode = Some("inline".to_string()),
                Some("file-only") => step.output_mode = Some("file-only".to_string()),
                _ => {
                    return Err(format!("config.steps[{i}].outputMode must be 'inline' or 'file-only'."))
                }
            }
        }
        if let Some(v) = obj.get("reads") {
            if v == &Value::Bool(false) {
                step.reads = Some(ChainListBinding::Toggle(false));
            } else if let Some(a) = v.as_array() {
                let list = a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                step.reads = Some(ChainListBinding::List(list));
            } else {
                return Err(format!("config.steps[{i}].reads must be an array or false."));
            }
        }
        if let Some(v) = obj.get("model") {
            match v.as_str() {
                Some(s) => step.model = Some(s.to_string()),
                None => return Err(format!("config.steps[{i}].model must be a string.")),
            }
        }
        if let Some(v) = obj.get("skills") {
            if v == &Value::Bool(false) {
                step.skills = Some(ChainListBinding::Toggle(false));
            } else if let Some(a) = v.as_array() {
                let list = a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                step.skills = Some(ChainListBinding::List(list));
            } else {
                return Err(format!("config.steps[{i}].skills must be an array or false."));
            }
        }
        if let Some(v) = obj.get("progress") {
            match v.as_bool() {
                Some(b) => step.progress = Some(b),
                None => return Err(format!("config.steps[{i}].progress must be a boolean.")),
            }
        }
        steps.push(step);
    }
    Ok(steps)
}

// -------------------------------------------------------------------------------------------
// discovery-driven lookup helpers (pi findAgents/findChains/availableNames/nameExistsInScope/
// unknownChainAgents + resolveTarget)
// -------------------------------------------------------------------------------------------

/// pi `findAgents` (`agent-management.ts:99-106`): raw-or-sanitized exact-name match over the
/// management (disabled-inclusive) view, optionally narrowed to one scope, sorted by source label.
fn find_agents(d: &AgentDiscoveryResult, name: &str, scope: Option<AgentSource>) -> Vec<AgentDefinition> {
    let raw = name.trim();
    let sanitized = sanitize_name(raw);
    let mut matches: Vec<AgentDefinition> = d
        .agents
        .iter()
        .filter(|a| scope.is_none() || Some(a.source) == scope)
        .filter(|a| a.name == raw || a.name == sanitized)
        .cloned()
        .collect();
    matches.sort_by(|a, b| source_str(a.source).cmp(source_str(b.source)));
    matches
}

/// pi `findChains` (`agent-management.ts:108-114`).
fn find_chains(d: &AgentDiscoveryResult, name: &str, scope: Option<AgentSource>) -> Vec<ChainDefinition> {
    let raw = name.trim();
    let sanitized = sanitize_name(raw);
    let mut matches: Vec<ChainDefinition> = d
        .chains
        .iter()
        .filter(|c| scope.is_none() || Some(c.source) == scope)
        .filter(|c| c.name == raw || c.name == sanitized)
        .cloned()
        .collect();
    matches.sort_by(|a, b| source_str(a.source).cmp(source_str(b.source)));
    matches
}

/// pi `availableNames(cwd, "agent")` (`agent-management.ts:93-97`): unique, sorted runtime names.
fn available_agent_names(d: &AgentDiscoveryResult) -> Vec<String> {
    d.agents
        .iter()
        .map(|a| a.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn available_chain_names(d: &AgentDiscoveryResult) -> Vec<String> {
    d.chains
        .iter()
        .map(|c| c.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// pi `nameExistsInScope` (`agent-management.ts:116-125`): whether an agent OR chain with this
/// runtime name already exists in the given writable scope (excluding one path, used on rename).
fn name_exists_in_scope(
    d: &AgentDiscoveryResult,
    scope: AgentSource,
    name: &str,
    exclude: Option<&Path>,
) -> bool {
    for a in &d.agents {
        if a.source == scope && a.name == name && Some(a.file_path.as_path()) != exclude {
            return true;
        }
    }
    for c in &d.chains {
        if c.source == scope && c.name == name && Some(c.file_path.as_path()) != exclude {
            return true;
        }
    }
    false
}

/// pi `unknownChainAgents` (`agent-management.ts:131-135`): step agents that resolve to no known
/// agent name, unique and sorted. Dynamic (agent-less) steps are skipped.
fn unknown_chain_agents(d: &AgentDiscoveryResult, steps: &[ChainStepConfig]) -> Vec<String> {
    let known: HashSet<&str> = d.agents.iter().map(|a| a.name.as_str()).collect();
    let mut missing = BTreeSet::new();
    for step in steps {
        if let Some(agent) = &step.agent
            && !known.contains(agent.as_str())
        {
            missing.insert(agent.clone());
        }
    }
    missing.into_iter().collect()
}

/// Shared shape over the two writable-target kinds (agent/chain) so [`resolve_target`] is one
/// implementation.
trait MutableTarget: Clone {
    fn source(&self) -> AgentSource;
    fn file_path(&self) -> &Path;
}

impl MutableTarget for AgentDefinition {
    fn source(&self) -> AgentSource {
        self.source
    }
    fn file_path(&self) -> &Path {
        &self.file_path
    }
}

impl MutableTarget for ChainDefinition {
    fn source(&self) -> AgentSource {
        self.source
    }
    fn file_path(&self) -> &Path {
        &self.file_path
    }
}

#[derive(Clone, Copy)]
enum TargetKind {
    Agent,
    Chain,
}

impl TargetKind {
    fn cap(self) -> &'static str {
        match self {
            TargetKind::Agent => "Agent",
            TargetKind::Chain => "Chain",
        }
    }
    fn low(self) -> &'static str {
        match self {
            TargetKind::Agent => "agent",
            TargetKind::Chain => "chain",
        }
    }
}

/// pi `resolveTarget` (`agent-management.ts:419-444`): pick the single writable target for a
/// mutating action, producing pi's exact read-only / not-found / disambiguation messages as an
/// error [`ManagementOutcome`].
fn resolve_target<T: MutableTarget>(
    kind: TargetKind,
    name: &str,
    matches: Vec<T>,
    available: &[String],
    scope_hint_raw: Option<&str>,
) -> Result<T, ManagementOutcome> {
    let mutable: Vec<T> = matches
        .iter()
        .filter(|m| m.source().is_writable())
        .cloned()
        .collect();
    if mutable.is_empty() {
        if !matches.is_empty() {
            return Err(ManagementOutcome::err(format!(
                "{} '{}' is read-only and cannot be modified. Create a same-named {} in user or project scope to override it.",
                kind.cap(),
                name,
                kind.low()
            )));
        }
        let avail = if available.is_empty() {
            "none".to_string()
        } else {
            available.join(", ")
        };
        return Err(ManagementOutcome::err(format!(
            "{} '{}' not found. Available: {}.",
            kind.cap(),
            name,
            avail
        )));
    }
    if mutable.len() == 1 {
        return mutable
            .into_iter()
            .next()
            .ok_or_else(|| ManagementOutcome::err("internal error: empty mutable set".to_string()));
    }
    let Some(scope) = disambiguation_scope(scope_hint_raw) else {
        let paths: Vec<String> = mutable
            .iter()
            .map(|m| format!("{}: {}", source_str(m.source()), m.file_path().display()))
            .collect();
        return Err(ManagementOutcome::err(format!(
            "{} '{}' exists in both scopes. Specify agentScope: 'user' or 'project'.\n{}",
            kind.cap(),
            name,
            paths.join("\n")
        )));
    };
    let scoped: Vec<T> = mutable.into_iter().filter(|m| m.source() == scope).collect();
    if scoped.is_empty() {
        return Err(ManagementOutcome::err(format!(
            "{} '{}' not found in scope '{}'.",
            kind.cap(),
            name,
            source_str(scope)
        )));
    }
    if scoped.len() > 1 {
        let paths: Vec<String> = scoped
            .iter()
            .map(|m| m.file_path().display().to_string())
            .collect();
        return Err(ManagementOutcome::err(format!(
            "Multiple {}s named '{}' found in scope '{}': {}",
            kind.low(),
            name,
            source_str(scope),
            paths.join(", ")
        )));
    }
    scoped
        .into_iter()
        .next()
        .ok_or_else(|| ManagementOutcome::err("internal error: empty scoped set".to_string()))
}

// -------------------------------------------------------------------------------------------
// Renderers (pi formatAgentDetail / formatChainDetail / formatChainStepDetail, 463-537)
// -------------------------------------------------------------------------------------------

/// pi `formatAgentDetail` (`agent-management.ts:463-489`).
fn format_agent_detail(a: &AgentDefinition) -> String {
    let mut tools_out: Vec<String> = Vec::new();
    if let Some(tools) = &a.tools {
        for tool in tools {
            match tool {
                ToolRef::Builtin(n) | ToolRef::ExtensionPath(n) => tools_out.push(n.clone()),
                ToolRef::Mcp(_) => {}
            }
        }
        for tool in tools {
            if let ToolRef::Mcp(n) = tool {
                tools_out.push(format!("mcp:{n}"));
            }
        }
    }

    let mut lines: Vec<String> = vec![
        format!("Agent: {} ({})", a.name, source_str(a.source)),
        format!("Path: {}", a.file_path.display()),
        format!("Description: {}", a.description),
    ];
    if a.package_name.is_some() {
        lines.push(format!("Local name: {}", a.local_name));
        if let Some(pkg) = &a.package_name {
            lines.push(format!("Package: {pkg}"));
        }
    }
    if let Some(model) = &a.model {
        lines.push(format!("Model: {model}"));
    }
    if !a.fallback_models.is_empty() {
        lines.push(format!(
            "Fallback models: {}",
            a.fallback_models
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !tools_out.is_empty() {
        lines.push(format!("Tools: {}", tools_out.join(", ")));
    }
    if !a.skills.is_empty() {
        lines.push(format!("Skills: {}", a.skills.join(", ")));
    }
    lines.push(format!(
        "System prompt mode: {}",
        match a.system_prompt_mode {
            SystemPromptMode::Append => "append",
            SystemPromptMode::Replace => "replace",
        }
    ));
    lines.push(format!(
        "Inherit project context: {}",
        if a.inherit_project_context { "true" } else { "false" }
    ));
    lines.push(format!(
        "Inherit skills: {}",
        if a.inherit_skills { "true" } else { "false" }
    ));
    if let Some(ctx) = a.default_context {
        lines.push(format!("Default context: {}", context_str(ctx)));
    }
    if a.source == AgentSource::Builtin {
        lines.push(format!(
            "Disabled: {}",
            if a.disabled.unwrap_or(false) { "true" } else { "false" }
        ));
    }
    if let Some(exts) = &a.extensions {
        lines.push(format!(
            "Extensions: {}",
            if exts.is_empty() { "(none)".to_string() } else { exts.join(", ") }
        ));
    }
    // pi renders `Subagent-only extensions` whenever the field is defined (even empty -> "(none)").
    // cyrup flattens it to a `Vec` with no defined/empty distinction, and its own serializer only
    // writes the key when non-empty, so a round-tripped file's non-empty <=> pi's "defined": render
    // only when non-empty (documented minor divergence limited to the defined-but-empty edge).
    if !a.subagent_only_extensions.is_empty() {
        lines.push(format!(
            "Subagent-only extensions: {}",
            a.subagent_only_extensions.join(", ")
        ));
    }
    if let Some(thinking) = &a.thinking {
        lines.push(format!("Thinking: {thinking}"));
    }
    if let Some(output) = &a.output
        && let Some(path) = &output.path
    {
        lines.push(format!("Output: {}", path.display()));
    }
    if let Some(reads) = &a.default_reads
        && !reads.is_empty()
    {
        lines.push(format!(
            "Reads: {}",
            reads
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if a.default_progress == Some(true) {
        lines.push("Progress: true".to_string());
    }
    if let Some(depth) = a.max_subagent_depth {
        lines.push(format!("Max subagent depth: {depth}"));
    }
    if a.completion_guard == Some(false) {
        lines.push("Completion guard: false".to_string());
    }
    if !a.system_prompt_body.trim().is_empty() {
        lines.push(String::new());
        lines.push("System Prompt:".to_string());
        lines.push(a.system_prompt_body.clone());
    }
    lines.join("\n")
}

/// pi `formatChainStepDetail` (`agent-management.ts:491-524`).
fn format_chain_step_detail(step: &ChainStepConfig, index: usize) -> Vec<String> {
    let n = index + 1;
    let mut lines: Vec<String> = Vec::new();
    if step.expand.is_some() || step.collect.is_some() {
        let collect_as = step
            .collect
            .as_ref()
            .and_then(|v| v.get("as"))
            .and_then(|v| v.as_str());
        lines.push(match collect_as {
            Some(a) => format!("{n}. Dynamic fanout -> {a}"),
            None => format!("{n}. Dynamic fanout"),
        });
        if let Some(expand) = &step.expand {
            if let Some(from) = expand.get("from") {
                let out = from.get("output").and_then(|v| v.as_str()).unwrap_or("?");
                let path = from.get("path").and_then(|v| v.as_str()).unwrap_or("");
                lines.push(format!("   Expand: {out}{path}"));
            }
            if let Some(item) = expand.get("item").and_then(|v| v.as_str()) {
                lines.push(format!("   Item variable: {item}"));
            }
            if let Some(key) = expand.get("key").and_then(|v| v.as_str()) {
                lines.push(format!("   Key: {key}"));
            }
            if let Some(max_items) = expand.get("maxItems").and_then(|v| v.as_i64()) {
                lines.push(format!("   Max items: {max_items}"));
            }
            if let Some(on_empty) = expand.get("onEmpty").and_then(|v| v.as_str()) {
                lines.push(format!("   On empty: {on_empty}"));
            }
        }
        if let Some(parallel) = &step.parallel {
            if let Some(agent) = parallel.get("agent").and_then(|v| v.as_str()) {
                lines.push(format!("   Agent: {agent}"));
            }
            if let Some(label) = parallel.get("label").and_then(|v| v.as_str()) {
                lines.push(format!("   Label: {label}"));
            }
            if let Some(task) = parallel.get("task").and_then(|v| v.as_str())
                && !task.trim().is_empty()
            {
                lines.push(format!("   Task: {task}"));
            }
            if parallel.get("outputSchema").is_some() {
                lines.push("   Structured output: true".to_string());
            }
        }
        if let Some(collect) = &step.collect
            && collect.get("outputSchema").is_some()
        {
            lines.push("   Collect schema: true".to_string());
        }
        if let Some(concurrency) = step.concurrency {
            lines.push(format!("   Concurrency: {concurrency}"));
        }
        if let Some(fail_fast) = step.fail_fast {
            lines.push(format!("   Fail fast: {}", if fail_fast { "true" } else { "false" }));
        }
        return lines;
    }

    lines.push(format!("{n}. {}", step.agent.as_deref().unwrap_or("")));
    if let Some(task) = &step.task
        && !task.trim().is_empty()
    {
        lines.push(format!("   Task: {task}"));
    }
    match &step.output {
        Some(ChainOutputBinding::Toggle(false)) => lines.push("   Output: false".to_string()),
        Some(ChainOutputBinding::Name(s)) => lines.push(format!("   Output: {s}")),
        _ => {}
    }
    if let Some(mode) = &step.output_mode {
        lines.push(format!("   Output mode: {mode}"));
    }
    match &step.reads {
        Some(ChainListBinding::Toggle(false)) => lines.push("   Reads: false".to_string()),
        Some(ChainListBinding::List(v)) if !v.is_empty() => {
            lines.push(format!("   Reads: {}", v.join(", ")))
        }
        _ => {}
    }
    if let Some(model) = &step.model {
        lines.push(format!("   Model: {model}"));
    }
    match &step.skills {
        Some(ChainListBinding::Toggle(false)) => lines.push("   Skills: false".to_string()),
        Some(ChainListBinding::List(v)) if !v.is_empty() => {
            lines.push(format!("   Skills: {}", v.join(", ")))
        }
        _ => {}
    }
    if let Some(progress) = step.progress {
        lines.push(format!("   Progress: {}", if progress { "true" } else { "false" }));
    }
    lines
}

/// pi `formatChainDetail` (`agent-management.ts:526-537`).
fn format_chain_detail(c: &ChainDefinition) -> String {
    let mut lines: Vec<String> = vec![
        format!("Chain: {} ({})", c.name, source_str(c.source)),
        format!("Path: {}", c.file_path.display()),
        format!("Description: {}", c.description),
    ];
    if c.package_name.is_some() {
        lines.push(format!("Local name: {}", c.local_name));
        if let Some(pkg) = &c.package_name {
            lines.push(format!("Package: {pkg}"));
        }
    }
    lines.push(String::new());
    lines.push("Steps:".to_string());
    for (i, step) in c.steps.iter().enumerate() {
        lines.extend(format_chain_step_detail(step, i));
    }
    lines.join("\n")
}

// -------------------------------------------------------------------------------------------
// handleList / handleGet / handleModels / handleCreate / handleUpdate / handleDelete
// -------------------------------------------------------------------------------------------

fn agent_in_list_scope(source: AgentSource, scope: Option<AgentSource>) -> bool {
    scope.is_none() || matches!(source, AgentSource::Builtin | AgentSource::Package) || Some(source) == scope
}

fn chain_in_list_scope(source: AgentSource, scope: Option<AgentSource>) -> bool {
    scope.is_none() || source == AgentSource::Package || Some(source) == scope
}

/// pi `handleList` (`agent-management.ts:539-566`). Proactive-skill and companion suggestion blocks
/// are deferred (skills/companion subsystems absent — see section header).
fn handle_list(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    let scope = normalize_list_scope(req.agent_scope);
    let d = discover_agents_all(cfg)?;

    let mut agents: Vec<&AgentDefinition> = d
        .agents
        .iter()
        .filter(|a| agent_in_list_scope(a.source, scope))
        .filter(|a| !a.disabled.unwrap_or(false))
        .collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));

    let mut chains: Vec<&ChainDefinition> = d
        .chains
        .iter()
        .filter(|c| chain_in_list_scope(c.source, scope))
        .collect();
    chains.sort_by(|a, b| a.name.cmp(&b.name));

    let diagnostics: Vec<&ChainDiscoveryDiagnostic> = d
        .diagnostics
        .iter()
        .filter(|e| scope.is_none() || Some(e.source) == scope)
        .collect();

    let mut lines: Vec<String> = Vec::new();
    lines.push("Executable agents:".to_string());
    if agents.is_empty() {
        lines.push("- (none)".to_string());
    } else {
        for a in &agents {
            let ctx = a
                .default_context
                .map(|c| format!(", context: {}", context_str(c)))
                .unwrap_or_default();
            lines.push(format!("- {} ({}{}): {}", a.name, source_str(a.source), ctx, a.description));
        }
    }
    lines.push(String::new());
    lines.push("Chains:".to_string());
    if chains.is_empty() {
        lines.push("- (none)".to_string());
    } else {
        for c in &chains {
            lines.push(format!("- {} ({}): {}", c.name, source_str(c.source), c.description));
        }
    }
    if !diagnostics.is_empty() {
        lines.push(String::new());
        lines.push("Chain diagnostics:".to_string());
        for e in &diagnostics {
            lines.push(format!("- {}: {}", e.file_path.display(), e.message));
        }
    }
    Ok(ManagementOutcome::ok(lines.join("\n")))
}

/// pi `handleGet` (`agent-management.ts:649-677`).
fn handle_get(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    if req.agent.is_none() && req.chain_name.is_none() {
        return Ok(ManagementOutcome::err("Specify 'agent' or 'chainName' for get."));
    }
    let has_both = req.agent.is_some() && req.chain_name.is_some();
    let d = discover_agents_all(cfg)?;
    let mut blocks: Vec<String> = Vec::new();
    let mut any_found = false;
    if let Some(agent_name) = req.agent {
        let matches = find_agents(&d, agent_name, None);
        if matches.is_empty() {
            let avail = available_agent_names(&d);
            let msg = format!(
                "Agent '{}' not found. Available: {}.",
                agent_name,
                if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
            );
            if !has_both {
                return Ok(ManagementOutcome::err(msg));
            }
            blocks.push(msg);
        } else {
            any_found = true;
            for a in &matches {
                blocks.push(format_agent_detail(a));
            }
        }
    }
    if let Some(chain_name) = req.chain_name {
        let matches = find_chains(&d, chain_name, None);
        if matches.is_empty() {
            let avail = available_chain_names(&d);
            let msg = format!(
                "Chain '{}' not found. Available: {}.",
                chain_name,
                if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
            );
            if !has_both {
                return Ok(ManagementOutcome::err(msg));
            }
            blocks.push(msg);
        } else {
            any_found = true;
            for c in &matches {
                blocks.push(format_chain_detail(c));
            }
        }
    }
    Ok(ManagementOutcome { text: blocks.join("\n\n"), is_error: !any_found })
}

/// Degraded port of pi `formatModelSource` (`agent-management.ts:568-578`). Live "inherits current
/// session model" resolution needs the model-registry/session handle that is `outer-layer` (Tier 8)
/// and not threaded into this crate; here we classify purely from discovery-time provenance
/// (`override_info` / `model_source`) and the agent's own resolved `model`.
fn format_model_source(agent: &AgentDefinition) -> String {
    if let Some(info) = &agent.override_info
        && agent.model != info.base_snapshot.model
    {
        return format!("{} override", override_scope_str(info.scope));
    }
    if matches!(agent.model_source, Some(AgentModelSourceInfo::SettingsDefault)) {
        return "settings defaultModel".to_string();
    }
    if agent.model.is_some() {
        return "builtin agent config".to_string();
    }
    "inherit requested, but no current session model is available".to_string()
}

/// pi `handleModels` (`agent-management.ts:580-647`), degraded: the live model registry + current
/// session model are `outer-layer` (Tier 8), so `Current session model` renders `(unavailable)` and
/// the effective model is the agent's own discovery-resolved `model` (or `(unresolved)`), classified
/// by [`format_model_source`]. The requested-filter validation, override provenance, and disabled
/// state are faithful.
fn handle_models(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    let requested = req.agent.map(str::trim).filter(|s| !s.is_empty());
    if let Some(name) = requested
        && !BUILTIN_AGENT_NAMES.contains(&name)
    {
        return Ok(ManagementOutcome::err(format!(
            "Builtin agent '{name}' not found. Available: {}.",
            BUILTIN_AGENT_NAMES.join(", ")
        )));
    }
    let d = discover_agents_all(cfg)?;
    let builtin_by_name: HashMap<&str, &AgentDefinition> = d
        .agents
        .iter()
        .filter(|a| a.source == AgentSource::Builtin)
        .map(|a| (a.name.as_str(), a))
        .collect();

    if let Some(name) = requested {
        let Some(agent) = builtin_by_name.get(name) else {
            return Ok(ManagementOutcome::err(format!("Builtin agent '{name}' not found.")));
        };
        let resolved = agent
            .model
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "(unresolved)".to_string());
        let mut lines = vec![
            "Builtin subagent model".to_string(),
            String::new(),
            format!("Agent: {name}"),
            "Effective model:".to_string(),
            format!("  {resolved}"),
            format!("Source: {}", format_model_source(agent)),
        ];
        if let Some(info) = &agent.override_info {
            lines.push("Override file:".to_string());
            lines.push(format!("  {}", info.settings_path.display()));
        }
        if agent.disabled == Some(true) {
            lines.push("Disabled: true".to_string());
        }
        lines.push("Current session model:".to_string());
        lines.push("  (unavailable)".to_string());
        return Ok(ManagementOutcome::ok(lines.join("\n")));
    }

    let mut lines = vec![
        "Builtin subagent models".to_string(),
        String::new(),
        "Current session model:".to_string(),
        "  (unavailable)".to_string(),
        String::new(),
    ];
    for name in BUILTIN_AGENT_NAMES {
        match builtin_by_name.get(name) {
            None => {
                lines.push(name.to_string());
                lines.push("  model:".to_string());
                lines.push("    (builtin definition not found)".to_string());
                lines.push("  source: missing".to_string());
                lines.push(String::new());
            }
            Some(agent) => {
                let resolved = agent
                    .model
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "(unresolved)".to_string());
                let source = format!(
                    "{}{}",
                    format_model_source(agent),
                    if agent.disabled == Some(true) { "; disabled" } else { "" }
                );
                lines.push(name.to_string());
                lines.push("  model:".to_string());
                lines.push(format!("    {resolved}"));
                lines.push(format!("  source: {source}"));
                lines.push(String::new());
            }
        }
    }
    Ok(ManagementOutcome::ok(lines.join("\n")))
}

/// The writable scope directory for a create, derived from the [`AgentDiscoveryConfig`] the same way
/// discovery scans it (so create and the next discovery pass agree on where the file lives).
///
/// The per-scope directory lists are ordered lowest-precedence-first (legacy `.agents` / extra dirs
/// early, the preferred `.cyrup/agents` — or the user's second `~/.agents` once it exists — last),
/// so the write target is the **last** (highest-precedence) entry: pi's `d.projectDir` = preferred
/// `<root>/.cyrup/agents` for a project create, and `d.userDir` = new-if-exists-else-old for a user
/// create (agent-management.ts:697-699, agents.ts:1420) — both the last entry under the topology
/// helpers' ordering. (For a single-entry list `first`/`last` coincide; only the multi-dir topology
/// distinguishes them.)
fn pick_scope_dir(cfg: &AgentDiscoveryConfig, scope: AgentSource, is_chain: bool) -> Option<PathBuf> {
    let dirs = match (scope, is_chain) {
        (AgentSource::User, false) => &cfg.user_agent_dirs,
        (AgentSource::User, true) => &cfg.user_chain_dirs,
        (AgentSource::Project, false) => &cfg.project_agent_dirs,
        (AgentSource::Project, true) => &cfg.project_chain_dirs,
        _ => return None,
    };
    dirs.last().cloned()
}

/// pi `handleCreate` (`agent-management.ts:679-738`). Model/skills registry warnings are deferred
/// (see section header); the create + name-collision + shadow-note + unknown-agent warnings are
/// faithful.
fn handle_create(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    use serde_json::Value;
    let cfg_map = match config_object(req.config) {
        Ok(Some(map)) => map,
        Ok(None) => return Ok(ManagementOutcome::err("config required for create.")),
        Err(e) => return Ok(ManagementOutcome::err(e)),
    };
    let name_raw = match cfg_map.get("name").and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => {
            return Ok(ManagementOutcome::err("config.name is required and must be a non-empty string."))
        }
    };
    let description = match cfg_map.get("description").and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => {
            return Ok(ManagementOutcome::err("config.description is required and must be a non-empty string."))
        }
    };
    let local_name = sanitize_name(&name_raw);
    if local_name.is_empty() {
        return Ok(ManagementOutcome::err("config.name is invalid after sanitization. Use letters, numbers, spaces, or hyphens."));
    }
    let package_name = match parse_package_config(cfg_map.get("package")) {
        Ok(pkg) => pkg,
        Err(e) => return Ok(ManagementOutcome::err(e)),
    };
    let runtime_name = AgentDefinition::qualified_name(&local_name, package_name.as_deref());
    let scope = match cfg_map.get("scope") {
        None => AgentSource::User,
        Some(Value::String(s)) if s == "user" => AgentSource::User,
        Some(Value::String(s)) if s == "project" => AgentSource::Project,
        _ => return Ok(ManagementOutcome::err("config.scope must be 'user' or 'project'.")),
    };
    let is_chain = cfg_map.contains_key("steps");
    let d = discover_agents_all(cfg)?;

    let Some(scope_dir) = pick_scope_dir(cfg, scope, is_chain) else {
        return Ok(ManagementOutcome::err(format!(
            "no {} {} directory is configured.",
            source_str(scope),
            if is_chain { "chain" } else { "agent" }
        )));
    };

    if name_exists_in_scope(&d, scope, &runtime_name, None) {
        return Ok(ManagementOutcome::err(format!(
            "Name '{runtime_name}' already exists in {} scope. Use update instead.",
            source_str(scope)
        )));
    }

    let mut warnings: Vec<String> = Vec::new();
    if !is_chain
        && d.agents
            .iter()
            .any(|a| a.source == AgentSource::Builtin && a.name == runtime_name)
    {
        warnings.push(format!("Note: this shadows the builtin agent '{runtime_name}'."));
    }

    if is_chain {
        let steps = match parse_step_list(cfg_map.get("steps")) {
            Ok(s) => s,
            Err(e) => return Ok(ManagementOutcome::err(e)),
        };
        let created = create_chain_with_steps(
            &scope_dir,
            scope,
            &local_name,
            package_name.clone(),
            &description,
            steps.clone(),
        )?;
        let missing = unknown_chain_agents(&d, &steps);
        if !missing.is_empty() {
            warnings.push(format!(
                "Warning: chain steps reference unknown agents: {}.",
                missing.join(", ")
            ));
        }
        let mut lines = vec![format!(
            "Created chain '{runtime_name}' at {}.",
            created.file_path.display()
        )];
        lines.extend(warnings);
        return Ok(ManagementOutcome::ok(lines.join("\n")));
    }

    let mut fields = AgentFields {
        system_prompt_mode: Some(default_system_prompt_mode(&local_name)),
        inherit_project_context: Some(default_inherit_project_context(&local_name)),
        inherit_skills: Some(false),
        package_name: Some(package_name.clone()),
        system_prompt_body: Some(String::new()),
        ..AgentFields::default()
    };
    if let Err(e) = apply_agent_config(&mut fields, &cfg_map) {
        return Ok(ManagementOutcome::err(e));
    }
    let Some(created) = create_agent(&scope_dir, scope, &local_name, &description, &fields)? else {
        // Pre-validated above via `parse_package_config`, so the low-level silent-skip path is
        // unreachable in practice; surface pi's own invalid-package text rather than panicking.
        return Ok(ManagementOutcome::err("config.package is invalid after sanitization."));
    };
    let mut lines = vec![format!(
        "Created agent '{runtime_name}' at {}.",
        created.file_path.display()
    )];
    lines.extend(warnings);
    Ok(ManagementOutcome::ok(lines.join("\n")))
}

/// The base definition to edit: pi `editableAgentConfig` (`agent-management.ts:174-196`) un-applies a
/// settings override so an update writes the agent's own base values, never the override-applied
/// ones. Settings overrides are inert today (C2), so `override_info` is always `None` and this is a
/// clone — kept forward-compatible for the moment C2 lands.
fn editable_base(target: &AgentDefinition) -> AgentDefinition {
    match &target.override_info {
        Some(info) => (*info.base_snapshot).clone(),
        None => target.clone(),
    }
}

/// pi `handleUpdate` (`agent-management.ts:740-847`). Model/fallback/skills registry warnings are
/// deferred (see section header); rename, package repackaging, unknown-agent warnings, and the
/// still-referenced-after-rename warning are faithful.
fn handle_update(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    use serde_json::Value;
    if req.agent.is_none() && req.chain_name.is_none() {
        return Ok(ManagementOutcome::err("Specify 'agent' or 'chainName' for update."));
    }
    if req.agent.is_some() && req.chain_name.is_some() {
        return Ok(ManagementOutcome::err("Specify either 'agent' or 'chainName', not both."));
    }
    let cfg_map = match config_object(req.config) {
        Ok(Some(map)) => map,
        Ok(None) => return Ok(ManagementOutcome::err("config required for update.")),
        Err(e) => return Ok(ManagementOutcome::err(e)),
    };
    let scope_hint = disambiguation_scope(req.agent_scope);

    if let Some(agent_name) = req.agent {
        let d = discover_agents_all(cfg)?;
        let matches = find_agents(&d, agent_name, scope_hint);
        let available = available_agent_names(&d);
        let target = match resolve_target(TargetKind::Agent, agent_name, matches, &available, req.agent_scope) {
            Ok(t) => t,
            Err(outcome) => return Ok(outcome),
        };
        if cfg_map.contains_key("name")
            && !matches!(cfg_map.get("name").and_then(Value::as_str), Some(s) if !s.trim().is_empty())
        {
            return Ok(ManagementOutcome::err("config.name must be a non-empty string when provided."));
        }
        if cfg_map.contains_key("description")
            && !matches!(cfg_map.get("description").and_then(Value::as_str), Some(s) if !s.trim().is_empty())
        {
            return Ok(ManagementOutcome::err("config.description must be a non-empty string when provided."));
        }
        let old_name = target.name.clone();
        let mut new_local = target.local_name.clone();
        if cfg_map.contains_key("name") {
            new_local = sanitize_name(cfg_map.get("name").and_then(Value::as_str).unwrap_or(""));
            if new_local.is_empty() {
                return Ok(ManagementOutcome::err("config.name is invalid after sanitization."));
            }
        }
        let mut new_pkg = target.package_name.clone();
        if cfg_map.contains_key("package") {
            match parse_package_config(cfg_map.get("package")) {
                Ok(pkg) => new_pkg = pkg,
                Err(e) => return Ok(ManagementOutcome::err(e)),
            }
        }
        let mut fields = AgentFields::default();
        if let Err(e) = apply_agent_config(&mut fields, &cfg_map) {
            return Ok(ManagementOutcome::err(e));
        }
        fields.local_name = Some(new_local.clone());
        fields.package_name = Some(new_pkg.clone());
        if cfg_map.contains_key("description") {
            fields.description = Some(
                cfg_map
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            );
        }
        let base = editable_base(&target);
        let Some(updated) = update_agent(&base, &fields)? else {
            return Ok(ManagementOutcome::err("config.package is invalid after sanitization."));
        };
        let new_runtime = updated.definition.name.clone();
        let final_outcome = if new_runtime != old_name {
            rename_agent(&updated.definition, &new_local)?
        } else {
            updated
        };
        let mut warnings: Vec<String> = Vec::new();
        if new_runtime != old_name {
            let refs: Vec<String> = discover_agents_all(cfg)?
                .chains
                .iter()
                .filter(|c| c.steps.iter().any(|s| s.agent.as_deref() == Some(old_name.as_str())))
                .map(|c| format!("{} ({})", c.name, source_str(c.source)))
                .collect();
            if !refs.is_empty() {
                warnings.push(format!(
                    "Warning: chains still reference '{old_name}': {}.",
                    refs.join(", ")
                ));
            }
        }
        let headline = if new_runtime == old_name {
            format!("Updated agent '{new_runtime}' at {}.", final_outcome.file_path.display())
        } else {
            format!(
                "Updated agent '{old_name}' to '{new_runtime}' at {}.",
                final_outcome.file_path.display()
            )
        };
        let mut lines = vec![headline];
        lines.extend(warnings);
        return Ok(ManagementOutcome::ok(lines.join("\n")));
    }

    // Chain update.
    let chain_name = req.chain_name.unwrap_or_default();
    let d = discover_agents_all(cfg)?;
    let matches = find_chains(&d, chain_name, scope_hint);
    let available = available_chain_names(&d);
    let target = match resolve_target(TargetKind::Chain, chain_name, matches, &available, req.agent_scope) {
        Ok(t) => t,
        Err(outcome) => return Ok(outcome),
    };
    if cfg_map.contains_key("name")
        && !matches!(cfg_map.get("name").and_then(Value::as_str), Some(s) if !s.trim().is_empty())
    {
        return Ok(ManagementOutcome::err("config.name must be a non-empty string when provided."));
    }
    if cfg_map.contains_key("description")
        && !matches!(cfg_map.get("description").and_then(Value::as_str), Some(s) if !s.trim().is_empty())
    {
        return Ok(ManagementOutcome::err("config.description must be a non-empty string when provided."));
    }
    let old_name = target.name.clone();
    let mut new_local = target.local_name.clone();
    if cfg_map.contains_key("name") {
        new_local = sanitize_name(cfg_map.get("name").and_then(Value::as_str).unwrap_or(""));
        if new_local.is_empty() {
            return Ok(ManagementOutcome::err("config.name is invalid after sanitization."));
        }
    }
    let mut new_pkg = target.package_name.clone();
    if cfg_map.contains_key("package") {
        match parse_package_config(cfg_map.get("package")) {
            Ok(pkg) => new_pkg = pkg,
            Err(e) => return Ok(ManagementOutcome::err(e)),
        }
    }
    let mut new_steps: Option<Vec<ChainStepConfig>> = None;
    if cfg_map.contains_key("steps") {
        match parse_step_list(cfg_map.get("steps")) {
            Ok(s) => new_steps = Some(s),
            Err(e) => return Ok(ManagementOutcome::err(e)),
        }
    }
    let new_description = if cfg_map.contains_key("description") {
        cfg_map
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    } else {
        target.description.clone()
    };
    let mut warnings: Vec<String> = Vec::new();
    let steps = match &new_steps {
        Some(ns) => {
            let missing = unknown_chain_agents(&d, ns);
            if !missing.is_empty() {
                warnings.push(format!(
                    "Warning: chain steps reference unknown agents: {}.",
                    missing.join(", ")
                ));
            }
            ns.clone()
        }
        None => target.steps.clone(),
    };
    let updated = update_chain_full(&target, &new_local, new_pkg.clone(), &new_description, steps)?;
    let headline = if updated.name == old_name {
        format!("Updated chain '{}' at {}.", updated.name, updated.file_path.display())
    } else {
        format!(
            "Updated chain '{old_name}' to '{}' at {}.",
            updated.name,
            updated.file_path.display()
        )
    };
    let mut lines = vec![headline];
    lines.extend(warnings);
    Ok(ManagementOutcome::ok(lines.join("\n")))
}

/// pi `handleDelete` (`agent-management.ts:849-868`).
fn handle_delete(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    if req.agent.is_none() && req.chain_name.is_none() {
        return Ok(ManagementOutcome::err("Specify 'agent' or 'chainName' for delete."));
    }
    if req.agent.is_some() && req.chain_name.is_some() {
        return Ok(ManagementOutcome::err("Specify either 'agent' or 'chainName', not both."));
    }
    let scope_hint = disambiguation_scope(req.agent_scope);
    if let Some(agent_name) = req.agent {
        let d = discover_agents_all(cfg)?;
        let matches = find_agents(&d, agent_name, scope_hint);
        let available = available_agent_names(&d);
        let target = match resolve_target(TargetKind::Agent, agent_name, matches, &available, req.agent_scope) {
            Ok(t) => t,
            Err(outcome) => return Ok(outcome),
        };
        delete_agent(&target)?;
        let refs: Vec<String> = discover_agents_all(cfg)?
            .chains
            .iter()
            .filter(|c| c.steps.iter().any(|s| s.agent.as_deref() == Some(target.name.as_str())))
            .map(|c| format!("{} ({})", c.name, source_str(c.source)))
            .collect();
        let mut lines = vec![format!("Deleted agent '{}' at {}.", target.name, target.file_path.display())];
        if !refs.is_empty() {
            lines.push(format!(
                "Warning: chains reference deleted agent '{}': {}.",
                target.name,
                refs.join(", ")
            ));
        }
        return Ok(ManagementOutcome::ok(lines.join("\n")));
    }
    let chain_name = req.chain_name.unwrap_or_default();
    let d = discover_agents_all(cfg)?;
    let matches = find_chains(&d, chain_name, scope_hint);
    let available = available_chain_names(&d);
    let target = match resolve_target(TargetKind::Chain, chain_name, matches, &available, req.agent_scope) {
        Ok(t) => t,
        Err(outcome) => return Ok(outcome),
    };
    delete_chain(&target)?;
    Ok(ManagementOutcome::ok(format!(
        "Deleted chain '{}' at {}.",
        target.name,
        target.file_path.display()
    )))
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
            local_name: "release".to_string(),
            package_name: None,
            description: "release chain".to_string(),
            source,
            file_path,
            steps: Vec::new(),
            extra_fields: BTreeMap::new(),
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
            thinking: Some(Some("high".to_string())),
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
        assert_eq!(outcome.definition.thinking, Some("high".to_string()));
        assert_eq!(outcome.definition.system_prompt_body, "You investigate things.");
    }

    #[test]
    fn serialize_agent_round_trip_preserves_unknown_keys_including_block_values() {
        // T7 §3: unknown keys survive a serialize -> re-parse round-trip, and a block-valued extra
        // field (embedded newlines, e.g. a `permission:` nested-YAML block) is re-emitted as an
        // indented block rather than corrupted into a flat line.
        use crate::discovery::frontmatter::parse_agent_file;

        let mut def = sample_agent(AgentSource::Project, PathBuf::from("/w.md"));
        def.local_name = "worker".to_string();
        def.name = "worker".to_string();
        def.description = "Worker".to_string();
        def.tools = Some(vec![ToolRef::Builtin("bash".to_string())]);
        def.thinking = Some("off".to_string());
        def.system_prompt_body = "Do work".to_string();
        let mut extra = BTreeMap::new();
        extra.insert("customVendorField".to_string(), "some-value".to_string());
        // `disabled:` in a file is an unknown extra field (T7 §2), so it round-trips here too.
        extra.insert("disabled".to_string(), "true".to_string());
        extra.insert(
            "permission".to_string(),
            "\"*\": ask\nread: allow\nbash:\n  \"*\": ask\n  \"git *\": allow".to_string(),
        );
        def.extra_fields = extra;

        // A preserve set pinning `thinking` reproduces an update that keeps the file's existing
        // `thinking: off` line (an explicit off is only emitted under preserve).
        let mut preserve = HashSet::new();
        preserve.insert("thinking".to_string());
        let serialized = serialize_agent(&def, Some(&preserve));

        assert!(
            serialized.contains(
                "permission:\n  \"*\": ask\n  read: allow\n  bash:\n    \"*\": ask\n    \"git *\": allow"
            ),
            "block-valued extra field must round-trip as an indented block:\n{serialized}"
        );
        assert!(serialized.contains("customVendorField: some-value"), "{serialized}");
        assert!(
            serialized.contains("disabled: true"),
            "disabled must round-trip as an extra field, not vanish:\n{serialized}"
        );
        assert!(
            serialized.contains("thinking: off"),
            "explicit off must survive under preserve:\n{serialized}"
        );

        let reparsed = parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))
            .expect("re-parses");
        assert_eq!(
            reparsed.extra_fields.get("customVendorField").map(String::as_str),
            Some("some-value")
        );
        assert_eq!(
            reparsed.extra_fields.get("permission").map(String::as_str),
            Some("\"*\": ask\nread: allow\nbash:\n  \"*\": ask\n  \"git *\": allow"),
            "the block value must round-trip byte-for-byte"
        );
        assert_eq!(reparsed.extra_fields.get("disabled").map(String::as_str), Some("true"));
        assert_eq!(reparsed.disabled, None, "disabled: in a file is never an honored flag");
        assert_eq!(reparsed.thinking, Some("off".to_string()));
    }

    #[test]
    fn update_preserves_unknown_frontmatter_key_and_explicit_off_on_disk() {
        // T7 §3 end-to-end: an update that changes only `description` must NOT drop an unknown key
        // (`vendorTag`) or the file's explicit `thinking: off`, and must NOT add fields the file
        // never had (preserve-frontmatter-fields semantics).
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("worker.md");
        std::fs::write(
            &path,
            "---\nname: worker\ndescription: Worker\nvendorTag: keep-me\nthinking: off\n---\n\nDo work\n",
        )
        .expect("write");
        let existing = crate::discovery::frontmatter::parse_agent_file(
            &std::fs::read_to_string(&path).expect("read"),
            AgentSource::Project,
            &path,
        )
        .expect("parses");
        assert_eq!(existing.extra_fields.get("vendorTag").map(String::as_str), Some("keep-me"));
        assert_eq!(existing.thinking, Some("off".to_string()));

        let fields = AgentFields {
            description: Some("Worker v2".to_string()),
            ..AgentFields::default()
        };
        update_agent(&existing, &fields)
            .expect("no error")
            .expect("not skipped");

        let content = std::fs::read_to_string(&path).expect("read back");
        assert!(content.contains("description: Worker v2"), "{content}");
        assert!(content.contains("vendorTag: keep-me"), "unknown key must survive update:\n{content}");
        assert!(content.contains("thinking: off"), "explicit off preserved on update:\n{content}");
        // The file never declared systemPromptMode, so a preserve-aware update must not inject it.
        assert!(!content.contains("systemPromptMode:"), "must not add absent default fields:\n{content}");
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

    // -----------------------------------------------------------------------------------------
    // C3: management-action dispatch + renderers (handleList/handleGet/handleModels/handleCreate/
    // handleUpdate/handleDelete + formatAgentDetail/formatChainDetail) — real discovery over real
    // on-disk files + the 8 bundled builtins, no mocks.
    // -----------------------------------------------------------------------------------------

    fn mgmt_cfg(tmp: &Path) -> AgentDiscoveryConfig {
        AgentDiscoveryConfig {
            builtin_agents_dir: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")),
            user_agent_dirs: vec![tmp.join("user/agents")],
            user_chain_dirs: vec![tmp.join("user/chains")],
            project_agent_dirs: vec![tmp.join("project/agents")],
            project_chain_dirs: vec![tmp.join("project/chains")],
            ..AgentDiscoveryConfig::default()
        }
    }

    fn mreq<'a>(
        agent: Option<&'a str>,
        chain: Option<&'a str>,
        scope: Option<&'a str>,
        config: Option<&'a serde_json::Value>,
    ) -> ManagementRequest<'a> {
        ManagementRequest { agent, chain_name: chain, agent_scope: scope, config }
    }

    #[test]
    fn list_includes_builtins_and_discovered_with_pi_shape() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        create_agent(&cfg.user_agent_dirs[0], AgentSource::User, "my-user-agent", "A user agent", &AgentFields::default())
            .expect("no error")
            .expect("not skipped");
        create_agent(&cfg.project_agent_dirs[0], AgentSource::Project, "my-project-agent", "A project agent", &AgentFields::default())
            .expect("no error")
            .expect("not skipped");

        let out = handle_management_action(&cfg, "list", &mreq(None, None, None, None)).expect("list ok");
        assert!(!out.is_error);
        let t = out.text;
        // pi list header shape (`agent-management.ts:553-560`).
        assert!(t.contains("Executable agents:"), "{t}");
        assert!(t.contains("Chains:"), "{t}");
        // The 8 R-SA-132 builtins load from resources/agents alongside the discovered agents.
        assert!(t.contains("- reviewer (builtin"), "{t}");
        assert!(t.contains("- scout (builtin"), "{t}");
        // Discovered user/project agents render with the exact pi line shape.
        assert!(t.contains("- my-user-agent (user): A user agent"), "{t}");
        assert!(t.contains("- my-project-agent (project): A project agent"), "{t}");
        // No chains authored -> the empty-chains sentinel.
        assert!(t.contains("Chains:\n- (none)"), "{t}");
        // Agents section precedes the chains section.
        let agents_idx = t.find("Executable agents:").expect("has agents header");
        let chains_idx = t.find("Chains:").expect("has chains header");
        assert!(agents_idx < chains_idx);
    }

    #[test]
    fn list_scope_filter_narrows_to_project_but_keeps_builtins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        create_agent(&cfg.user_agent_dirs[0], AgentSource::User, "my-user-agent", "A user agent", &AgentFields::default())
            .expect("ok").expect("not skipped");
        create_agent(&cfg.project_agent_dirs[0], AgentSource::Project, "my-project-agent", "A project agent", &AgentFields::default())
            .expect("ok").expect("not skipped");

        let out = handle_management_action(&cfg, "list", &mreq(None, None, Some("project"), None)).expect("list ok");
        let t = out.text;
        assert!(t.contains("- my-project-agent (project)"), "{t}");
        assert!(!t.contains("- my-user-agent (user)"), "project scope must hide user agents: {t}");
        // Builtins remain visible under any named scope (they are orthogonal to the user/project axis).
        assert!(t.contains("- reviewer (builtin"), "{t}");
    }

    #[test]
    fn create_get_update_delete_round_trip_user_scope() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());

        let create_cfg = serde_json::json!({
            "name": "Recon Scout",
            "description": "Fast recon",
            "systemPrompt": "Inspect the tree.",
            "tools": "read, grep, ls"
        });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&create_cfg))).expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.starts_with("Created agent 'recon-scout' at "), "{}", created.text);
        let file = cfg.user_agent_dirs[0].join("recon-scout.md");
        assert!(file.exists());

        let got = handle_management_action(&cfg, "get", &mreq(Some("recon-scout"), None, None, None)).expect("get ok");
        assert!(!got.is_error, "{}", got.text);
        assert!(got.text.contains("Agent: recon-scout (user)"), "{}", got.text);
        assert!(got.text.contains("Description: Fast recon"), "{}", got.text);
        assert!(got.text.contains("Tools: read, grep, ls"), "{}", got.text);
        assert!(got.text.contains("System prompt mode: replace"), "{}", got.text);
        assert!(got.text.contains("System Prompt:\nInspect the tree."), "{}", got.text);

        let update_cfg = serde_json::json!({ "description": "Faster recon" });
        let updated = handle_management_action(&cfg, "update", &mreq(Some("recon-scout"), None, None, Some(&update_cfg))).expect("update ok");
        assert!(!updated.is_error, "{}", updated.text);
        assert!(updated.text.starts_with("Updated agent 'recon-scout' at "), "{}", updated.text);
        let got2 = handle_management_action(&cfg, "get", &mreq(Some("recon-scout"), None, None, None)).expect("get ok");
        assert!(got2.text.contains("Description: Faster recon"), "{}", got2.text);
        // The un-touched tools survive the merge-update (field-level patch, not a full replace).
        assert!(got2.text.contains("Tools: read, grep, ls"), "{}", got2.text);

        let deleted = handle_management_action(&cfg, "delete", &mreq(Some("recon-scout"), None, None, None)).expect("delete ok");
        assert!(!deleted.is_error, "{}", deleted.text);
        assert!(deleted.text.starts_with("Deleted agent 'recon-scout' at "), "{}", deleted.text);
        assert!(!file.exists());
    }

    #[test]
    fn create_and_delete_round_trip_project_scope_with_collision_guard() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let create_cfg = serde_json::json!({ "name": "proj-only", "description": "Project agent", "scope": "project" });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&create_cfg))).expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        let file = cfg.project_agent_dirs[0].join("proj-only.md");
        assert!(file.exists());

        // Re-create is rejected (name already exists in the same scope).
        let again = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&create_cfg))).expect("no discovery error");
        assert!(again.is_error);
        assert!(again.text.contains("already exists in project scope"), "{}", again.text);

        let deleted = handle_management_action(&cfg, "delete", &mreq(Some("proj-only"), None, None, None)).expect("delete ok");
        assert!(!deleted.is_error, "{}", deleted.text);
        assert!(!file.exists());
    }

    #[test]
    fn update_and_delete_reject_builtin_agents_with_read_only_message() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let upd_cfg = serde_json::json!({ "description": "hijack" });
        let upd = handle_management_action(&cfg, "update", &mreq(Some("reviewer"), None, None, Some(&upd_cfg))).expect("no discovery error");
        assert!(upd.is_error);
        assert!(upd.text.contains("Agent 'reviewer' is read-only and cannot be modified"), "{}", upd.text);

        let del = handle_management_action(&cfg, "delete", &mreq(Some("reviewer"), None, None, None)).expect("no discovery error");
        assert!(del.is_error);
        assert!(del.text.contains("Agent 'reviewer' is read-only and cannot be modified"), "{}", del.text);
        // The bundled builtin file was NOT removed.
        assert!(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/agents/reviewer.md").exists());
    }

    #[test]
    fn resolve_target_rejects_package_source_with_read_only_message() {
        // Package tier is not populated by discovery in a bare test cfg, so exercise the
        // management-layer read-only gate (pi resolveTarget) directly against a Package-sourced
        // match — the exact path a `subagent update/delete` on a packaged agent takes (R-SA-014).
        let mut pkg = sample_agent(AgentSource::Package, PathBuf::from("/pkg/acme.tool.md"));
        pkg.name = "acme.tool".to_string();
        let outcome = resolve_target(TargetKind::Agent, "acme.tool", vec![pkg], &[], None)
            .expect_err("a package-sourced target must be rejected as read-only");
        assert!(outcome.is_error);
        assert!(
            outcome.text.contains("Agent 'acme.tool' is read-only and cannot be modified"),
            "{}",
            outcome.text
        );
    }

    #[test]
    fn create_rejects_invalid_package_with_pi_message() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let bad = serde_json::json!({ "name": "Scout", "package": "!!!", "description": "x", "scope": "project" });
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&bad))).expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("config.package is invalid"), "{}", out.text);
    }

    #[test]
    fn create_rejects_non_boolean_completion_guard_with_exact_pi_message() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let bad = serde_json::json!({ "name": "test-runner", "description": "Run tests", "scope": "project", "completionGuard": "false" });
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&bad))).expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("config.completionGuard must be a boolean"), "{}", out.text);
    }

    #[test]
    fn create_surfaces_json_parse_errors_for_string_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let bad = serde_json::json!("{\"name\":");
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&bad))).expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("config must be valid JSON:"), "{}", out.text);
    }

    #[test]
    fn create_delegate_gets_name_sensitive_defaults_and_shadow_note() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let c = serde_json::json!({ "name": "delegate", "description": "Delegate helper", "scope": "project" });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&c))).expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.contains("shadows the builtin agent 'delegate'"), "{}", created.text);

        let got = handle_management_action(&cfg, "get", &mreq(Some("delegate"), None, None, None)).expect("get ok");
        // The custom project delegate wins over the builtin and shows delegate's name-sensitive defaults.
        assert!(got.text.contains("Agent: delegate (project)"), "{}", got.text);
        assert!(got.text.contains("System prompt mode: append"), "{}", got.text);
        assert!(got.text.contains("Inherit project context: true"), "{}", got.text);
        assert!(got.text.contains("Inherit skills: false"), "{}", got.text);
    }

    #[test]
    fn get_unknown_agent_is_a_not_found_error_listing_available() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "get", &mreq(Some("nope"), None, None, None)).expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("Agent 'nope' not found. Available: "), "{}", out.text);
        assert!(out.text.contains("reviewer"), "available list must include the builtins: {}", out.text);
    }

    #[test]
    fn create_chain_appears_in_list_and_get_renders_steps() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let chain_cfg = serde_json::json!({
            "name": "Review Flow",
            "description": "Scout then review",
            "scope": "project",
            "steps": [
                { "agent": "scout", "task": "Find targets" },
                { "agent": "reviewer", "task": "Review {previous}", "model": "fast" }
            ]
        });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&chain_cfg))).expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.starts_with("Created chain 'review-flow' at "), "{}", created.text);
        // scout + reviewer are builtins, so no unknown-agent warning is appended.
        assert!(!created.text.contains("unknown agents"), "{}", created.text);

        let listed = handle_management_action(&cfg, "list", &mreq(None, None, None, None)).expect("list ok");
        assert!(listed.text.contains("- review-flow (project): Scout then review"), "{}", listed.text);

        let got = handle_management_action(&cfg, "get", &mreq(None, Some("review-flow"), None, None)).expect("get ok");
        assert!(!got.is_error, "{}", got.text);
        assert!(got.text.contains("Chain: review-flow (project)"), "{}", got.text);
        assert!(got.text.contains("1. scout"), "{}", got.text);
        assert!(got.text.contains("   Task: Find targets"), "{}", got.text);
        assert!(got.text.contains("2. reviewer"), "{}", got.text);
        assert!(got.text.contains("   Model: fast"), "{}", got.text);
    }

    #[test]
    fn create_chain_warns_on_unknown_step_agents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let chain_cfg = serde_json::json!({
            "name": "mystery",
            "description": "refs a ghost",
            "scope": "user",
            "steps": [ { "agent": "ghost-agent", "task": "boo" } ]
        });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&chain_cfg))).expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.contains("Warning: chain steps reference unknown agents: ghost-agent."), "{}", created.text);
    }

    #[test]
    fn models_lists_builtin_mapping_without_a_live_registry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "models", &mreq(None, None, None, None)).expect("models ok");
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Builtin subagent models"), "{}", out.text);
        for name in BUILTIN_AGENT_NAMES {
            assert!(out.text.contains(name), "missing builtin {name}: {}", out.text);
        }
        // The live registry / session model is outer-layer (Tier 8) — documented degradation.
        assert!(out.text.contains("Current session model:\n  (unavailable)"), "{}", out.text);
    }

    #[test]
    fn models_rejects_unknown_builtin_filter() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "models", &mreq(Some("not-a-builtin"), None, None, None)).expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("Builtin agent 'not-a-builtin' not found"), "{}", out.text);
    }

    #[test]
    fn unknown_action_is_reported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "frobnicate", &mreq(None, None, None, None)).expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("Unknown action: frobnicate"), "{}", out.text);
    }

    #[test]
    fn get_renders_packaged_agent_local_name_and_package() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let create_cfg = serde_json::json!({
            "name": "Scout",
            "package": "Code Analysis",
            "description": "Fast recon",
            "scope": "project"
        });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&create_cfg))).expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.starts_with("Created agent 'code-analysis.scout' at "), "{}", created.text);

        let got = handle_management_action(&cfg, "get", &mreq(Some("code-analysis.scout"), None, None, None)).expect("get ok");
        assert!(got.text.contains("Agent: code-analysis.scout (project)"), "{}", got.text);
        assert!(got.text.contains("Local name: scout"), "{}", got.text);
        assert!(got.text.contains("Package: code-analysis"), "{}", got.text);
    }
}
