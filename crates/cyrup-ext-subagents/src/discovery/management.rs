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
use super::{
    discover_agents_all, resolve_agent_name, AgentDiscoveryConfig, AgentDiscoveryResult,
    AgentNameResolution,
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
    /// pi `config.aliases` (`agent-management.ts:411-421` @ v0.43.0). `Some(list)` sets the alias
    /// list (already normalized against the target's name); `Some(vec![])` is the `false`/`""`
    /// CLEAR; `None` means the update config never mentioned `aliases` and the existing list stands.
    pub aliases: Option<Vec<String>>,
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
        aliases: fields.aliases.clone().unwrap_or_default(),
        tools: fields.tools.clone().unwrap_or(None),
        extensions: fields.extensions.clone().unwrap_or(None),
        extensions_from_default: false,
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
        // Not exposed as an editable management field (pi's `agentCreate`/`agentUpdate` input
        // schemas have no `memory`/`toolBudget` key either); a created agent declares neither.
        default_async: None,
        default_timeout_ms: None,
        memory: None,
        tool_budget: None,
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
        aliases: fields.aliases.clone().unwrap_or_else(|| existing.aliases.clone()),
        tools: fields.tools.clone().unwrap_or_else(|| existing.tools.clone()),
        extensions: fields
            .extensions
            .clone()
            .unwrap_or_else(|| existing.extensions.clone()),
        extensions_from_default: existing.extensions_from_default,
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
        // An UPDATE never edits these two (no management field exists for them) but must not
        // DROP them either — an agent file with a `memory:`/`toolBudget:` block that is renamed
        // or field-edited keeps both, exactly as pi's preserve-frontmatter update does.
        default_async: existing.default_async,
        default_timeout_ms: existing.default_timeout_ms,
        memory: existing.memory.clone(),
        tool_budget: existing.tool_budget.clone(),
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
// pre-override `editable_base` snapshot (pi `editableAgentConfig`, `agent-management.ts:217-267`),
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
    // aliases (`agent-serializer.ts:59-60` @ v0.43.0):
    // `if (aliasesValue || preserve("alias", "aliases")) lines.push(`aliases: ${aliasesValue ?? ""}`)`.
    // Both spellings are `KNOWN_FIELDS`, so this line is what stops a management rewrite from
    // silently DELETING an author's `alias:`/`aliases:` (the extra-fields loop skips known keys);
    // an author who wrote the singular `alias:` gets the plural back, exactly as upstream does.
    {
        let aliases_value =
            if def.aliases.is_empty() { None } else { Some(def.aliases.join(", ")) };
        if aliases_value.is_some() || preserve(&["alias", "aliases"]) {
            lines.push(format!("aliases: {}", aliases_value.as_deref().unwrap_or("")));
        }
    }

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

    // async / timeoutMs launch defaults (`agent-serializer.ts:87-88` @ v0.43.0): emitted whenever
    // set, or as an empty value under preserve. Same silent-deletion trap as `toolBudget` below —
    // a KNOWN_FIELD the serializer never writes is dropped on the first management rewrite.
    if def.default_async.is_some() || preserve(&["async"]) {
        let value = match def.default_async {
            None => "",
            Some(true) => "true",
            Some(false) => "false",
        };
        lines.push(format!("async: {value}"));
    }
    if def.default_timeout_ms.is_some() || preserve(&["timeoutMs"]) {
        let value = def
            .default_timeout_ms
            .map_or_else(String::new, |ms| ms.to_string());
        lines.push(format!("timeoutMs: {value}"));
    }

    // toolBudget (`agent-serializer.ts:91-93` @ v0.34.0): emitted as compact JSON when set, or as
    // an empty value under preserve. Without this, adding `toolBudget` to `KNOWN_FIELDS` would let
    // any management update SILENTLY DELETE an author's budget — the extra-fields loop below skips
    // known keys, so a field that is known but never emitted simply vanishes on rewrite.
    if def.tool_budget.is_some() || preserve(&["toolBudget"]) {
        let value = def
            .tool_budget
            .as_ref()
            .and_then(|b| serde_json::to_string(b).ok())
            .unwrap_or_default();
        lines.push(format!("toolBudget: {value}"));
    }

    // memory (`agent-serializer.ts:95-99` @ v0.34.0): a two-line nested block, emitted ONLY when
    // set (upstream has no `preserve` arm for it).
    if let Some(memory) = &def.memory {
        let scope = match memory.scope {
            crate::discovery::types::MemoryScope::Project => "project",
            crate::discovery::types::MemoryScope::User => "user",
        };
        lines.push("memory:".to_string());
        lines.push(format!("  scope: {scope}"));
        lines.push(format!("  path: {}", memory.path));
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

/// pi `preservedAgentFrontmatterFields` (`agent-management.ts:278-330`): starting from the field
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
    // pi `agent-management.ts:287` @ v0.43.0: `if (hasKey(cfg, "aliases")) changed("alias", "aliases")`
    // — an update that sets `aliases` un-preserves BOTH spellings so the new value is serialized.
    if fields.aliases.is_some() {
        set.remove("alias");
        set.remove("aliases");
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
/// `handleCreate`'s chain branch, `agent-management.ts:935-952`). Unlike the bare [`create_chain`]
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
/// `agent-management.ts:1041-1087`). Unlike the bare [`update_chain`] skeleton (which replaces the
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
/// `chain-serializer.ts:228-241`): a root object with the pre-qualification `name`
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
//   - model-registry warnings (`modelWarning`/`fallbackModelsWarning`) in `models` — still need the
//     live model-registry/catalog handle to validate a referenced model actually exists; that
//     catalog probe remains deferred. Live SESSION-model resolution, however, is now wired: the
//     parent session model (pi `ctx.model`) is threaded in via
//     [`ManagementRequest::current_session_model`] (from
//     [`cyrup_ext::host::HostServices::current_model`]), so an inheriting persona's effective model
//     + the `Current session model` line render the real `provider/id` (see [`format_model_source`]),
//     degrading to `(unavailable)` only with no live session.
//   - skill warnings (`skillsWarning`) and proactive-skill suggestions in `list` — need the skills
//     subsystem (C4 / Tier 5), entirely absent today.
//   - settings-override un-apply on update (`editableAgentConfig`) — settings overrides are inert
//     today (C2 / Tier 2), so `override_info` is always `None` and the un-apply ([`editable_base`])
//     is a no-op; it is still applied forward-compatibly here so the moment C2 lands it is correct.
//
// One architectural divergence (documented, NOT a management-layer bug): pi's `discoverAgentsAll`
// returns UNMERGED per-tier arrays (`agents.ts:1783-1888`), so `list`/`get` on a name that a
// user/project agent shadows across tiers show BOTH the builtin/package entry AND the shadowing
// entry. cyrup's `discover_agents_all` returns the R-SA-001 four-tier MERGE (one precedence-winner
// per name, by deliberate architecture), so `list`/`get` show only the winner. `update`/`delete`
// outcomes are UNAFFECTED — resolveTarget's mutable winner is identical either way. Reproducing pi's
// raw-tier duplicate view would require a separate unmerged discovery entry point (Tier-7 discovery
// scope), out of this C3 task. In the common (non-shadowing) case the output is byte-identical.
// ===============================================================================================

/// pi's `BUILTIN_AGENT_NAMES` (`agents.ts:38-46` @ v0.43.0) — used by [`handle_models`] to bound the
/// requested filter and to iterate the builtin model mapping in pi's exact stable order.
///
/// SEVEN names, not eight. Upstream `83b9872` ("fix: remove stale bundled roles") deleted the
/// `planner` and `context-builder` roles outright — their `agents/*.md`, their paired prompt
/// templates, and every special case keyed on their names — and `bff9722` added `advisor`, which
/// `34a018f` then demoted from its own `agents/advisor.md` to an ALIAS on `oracle`
/// (`agents/oracle.md:3` @ v0.43.0 carries `aliases: advisor`). `advisor` therefore stays in this
/// list — the roster is the set of names the model-report surface enumerates, and pi keeps listing
/// it — while shipping NO `advisor.md` of its own; the alias is what resolves it.
pub const BUILTIN_AGENT_NAMES: [&str; 7] = [
    "advisor",
    "delegate",
    "oracle",
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
    /// The live PARENT session model (`provider/id`, from
    /// [`cyrup_ext::host::HostServices::current_model`] — pi's `ctx.model`), threaded in by the
    /// caller so [`handle_models`]'s `Current session model` line + `formatModelSource`'s inherit
    /// branch render the REAL inherited model instead of `(unavailable)`. `None` (no live session
    /// backend bound / headless) keeps the genuine no-host degrade. Only the `models` action reads
    /// it; the other handlers ignore it.
    pub current_session_model: Option<&'a str>,
    /// The proactive skill-subagent inputs [`handle_list`] splices in — pi's
    /// `ctx.config?.proactiveSkillSubagents` plus its `discoverAvailableSkills: () =>
    /// discoverAvailableSkills(ctx.cwd)` closure (`agent-management.ts:765-770` @v0.43.0). `None`
    /// means the caller performed no availability scan, which yields no suggestions — the same
    /// outcome upstream reaches when its `discoverAvailableSkills` throws
    /// (`proactive-skills.ts:182-186` catches to `[]`, and an empty availability list matches no
    /// skill). Only the `list` action reads it.
    pub proactive_skills: Option<ProactiveSkillsInput<'a>>,
}

/// The two proactive skill-subagent inputs `handleList` reads off its `ManagementContext`
/// (`agent-management.ts:765-770` @v0.43.0), carried on [`ManagementRequest`] because cyrup's
/// management layer takes a request rather than a context object.
///
/// **Why the availability list is pre-resolved rather than a closure.** Upstream passes a lazy
/// `discoverAvailableSkills: () => AvailableSkill[]` so that a disabled feature performs no
/// filesystem scan. cyrup's [`crate::discovery::skills::discover_available_skills`] is `async`
/// while [`handle_management_action`] is sync, so the laziness moves one level up: the async caller
/// checks [`crate::discovery::skills::resolve_proactive_skill_subagents_config`]'s `enabled` first
/// and only then awaits the scan, filling this field. Both upstream properties survive — no scan
/// when disabled, and no suggestions when the scan found nothing.
pub struct ProactiveSkillsInput<'a> {
    /// pi `ctx.config?.proactiveSkillSubagents`. `None` is pi's `undefined` (defaults-on).
    pub setting: Option<&'a crate::discovery::skills::ProactiveSkillSubagentsSetting>,
    /// The already-resolved result of pi's `discoverAvailableSkills(ctx.cwd)` closure.
    pub available_skills: &'a [crate::discovery::skills::AvailableSkill],
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

/// The ten management actions [`handle_management_action`] dispatches — pi's `ManagementAction`
/// union (`shared/types.ts`), in pi's own declaration order. Exposed so `extension.rs`'s tool schema
/// and its child-safe mutating-action denylist are derived from ONE list rather than three hand-kept
/// copies that can drift apart.
pub const MANAGEMENT_ACTIONS: [&str; 10] = [
    "list", "get", "models", "create", "update", "delete", "eject", "disable", "enable", "reset",
];

/// pi `MUTATING_MANAGEMENT_ACTIONS` (`runs/foreground/subagent-executor.ts:112`): the management
/// actions a child-safe (fanout) tool registration must refuse. `list`/`get`/`models` are read-only
/// and stay permitted; the other seven all write to the parent's on-disk agent config — the four
/// SUBA-005 additions (`eject` writes an agent file, `disable`/`enable`/`reset` write
/// `settings.json`) are mutations exactly as much as `create`/`update`/`delete` are, so they join
/// the same denylist rather than sneaking through as "just management".
pub const MUTATING_MANAGEMENT_ACTIONS: [&str; 7] =
    ["create", "update", "delete", "eject", "disable", "enable", "reset"];

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
        // SUBA-005 (pi `agent-management.ts:1046-1049`): the tier-aware / settings-writing four.
        "eject" => handle_eject(cfg, req),
        "disable" => handle_disable(cfg, req),
        "enable" => handle_enable(cfg, req),
        "reset" => handle_reset(cfg, req),
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
    target_name: &str,
) -> Result<(), String> {
    use serde_json::Value;

    // pi `agent-management.ts:411-421` @ v0.43.0 — the FIRST branch of `applyAgentConfig`:
    //
    //   if (cfg.aliases === false || cfg.aliases === "") target.aliases = undefined;
    //   else if (typeof cfg.aliases === "string") { parseCsv(...).filter(a => a !== target.name) }
    //   else if (Array.isArray(...) && every string) { [...new Set(map(trim).filter(Boolean))].filter(a => a !== target.name) }
    //   else return "config.aliases must be a comma-separated string, string array, or false when provided.";
    //
    // `target_name` is the target's name AS IT STANDS WHEN THE CONFIG IS APPLIED, which on a rename
    // is still the OLD runtime name: upstream calls `applyAgentConfig` at `:1006`, six lines before
    // `updated.name = buildRuntimeName(newLocalName, newPackageName)` at `:1012`.
    if let Some(v) = cfg.get("aliases") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.aliases = Some(Vec::new());
        } else if let Some(raw) = v.as_str() {
            fields.aliases =
                Some(parse_csv(raw).into_iter().filter(|a| a != target_name).collect());
        } else if let Some(arr) = v.as_array()
            && arr.iter().all(Value::is_string)
        {
            let mut seen = HashSet::new();
            let mut aliases = Vec::new();
            for item in arr {
                let trimmed = item.as_str().unwrap_or_default().trim();
                if trimmed.is_empty() || trimmed == target_name {
                    continue;
                }
                if seen.insert(trimmed.to_string()) {
                    aliases.push(trimmed.to_string());
                }
            }
            fields.aliases = Some(aliases);
        } else {
            return Err(
                "config.aliases must be a comma-separated string, string array, or false when provided."
                    .to_string(),
            );
        }
    }
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

/// pi `findAgents` (`agent-management.ts:114-126` @ v0.43.0): ALIAS-AWARE lookup over the management
/// (disabled-inclusive) view, optionally narrowed to one scope, sorted by source label.
///
/// The upstream shape, verbatim:
/// 1. Resolve `raw` against the scoped list with [`resolve_agent_name`].
/// 2. If that neither matched nor was ambiguous, retry with the sanitized name (only when the
///    sanitized form actually differs). An AMBIGUOUS first attempt is NOT retried — the retry guard
///    is `!resolved.agent && !resolved.error`.
/// 3. On a hit, return EVERY definition sharing the resolved CANONICAL name (so a user file
///    shadowing a builtin still yields both tiers, which is what `resolve_target`'s
///    both-scopes/read-only messages are built on).
/// 4. On a miss OR an ambiguity, fall back to the per-candidate membership probe
///    `resolveAgentName(raw, [agent]).agent` — which, run against a ONE-element list, can never be
///    ambiguous, so this is exactly "every agent whose own name/localName/aliases answer to `raw`
///    (or to the sanitized form)". That is what surfaces the several distinct canonical names an
///    ambiguity error must list.
fn find_agents(d: &AgentDiscoveryResult, name: &str, scope: Option<AgentSource>) -> Vec<AgentDefinition> {
    let raw = name.trim();
    let sanitized = sanitize_name(raw);
    let scoped: Vec<AgentDefinition> = d
        .agents
        .iter()
        .filter(|a| scope.is_none() || Some(a.source) == scope)
        .cloned()
        .collect();

    let mut resolved = resolve_agent_name(raw, &scoped);
    if matches!(resolved, AgentNameResolution::NotFound) && sanitized != raw {
        resolved = resolve_agent_name(&sanitized, &scoped);
    }

    let mut matches: Vec<AgentDefinition> = if let Some(agent) = resolved.agent() {
        let canonical = agent.name.clone();
        scoped.iter().filter(|a| a.name == canonical).cloned().collect()
    } else {
        scoped
            .iter()
            .filter(|a| {
                let one = std::slice::from_ref(*a);
                resolve_agent_name(raw, one).agent().is_some()
                    || (sanitized != raw
                        && resolve_agent_name(&sanitized, one).agent().is_some())
            })
            .cloned()
            .collect()
    };
    matches.sort_by(|a, b| source_str(a.source).cmp(source_str(b.source)));
    matches
}

/// The DISTINCT canonical names present in a match set, sorted — pi's
/// `[...new Set(matches.map(m => m.name))].sort((a, b) => a.localeCompare(b))`
/// (`agent-management.ts:624-626,880-882`). More than one entry means the requested name/alias is
/// ambiguous and every caller must refuse rather than pick.
fn distinct_agent_names<'a>(matches: impl IntoIterator<Item = &'a AgentDefinition>) -> Vec<String> {
    matches.into_iter().map(|a| a.name.clone()).collect::<BTreeSet<_>>().into_iter().collect()
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
    // pi v0.43.0 (`agent-management.ts:169-174`) replaced the `new Set(allAgents(d).map(a => a.name))`
    // membership test with `!resolveAgentName(agentName, agents).agent`, so a step that names an
    // ALIAS is known and no longer warns. An ambiguous name yields no `.agent` and is therefore
    // reported as unknown — upstream's behaviour, and defensible: the chain cannot be run either way.
    let mut missing = BTreeSet::new();
    for step in steps {
        if let Some(agent) = &step.agent
            && resolve_agent_name(agent, &d.agents).agent().is_none()
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
    /// The target's CANONICAL name — pi widened `resolveTarget`'s bound to
    /// `T extends { name: string; … }` (`agent-management.ts:617`) precisely so it could reject a
    /// match set spanning several distinct names.
    fn target_name(&self) -> &str;
}

impl MutableTarget for AgentDefinition {
    fn source(&self) -> AgentSource {
        self.source
    }
    fn file_path(&self) -> &Path {
        &self.file_path
    }
    fn target_name(&self) -> &str {
        &self.name
    }
}

impl MutableTarget for ChainDefinition {
    fn source(&self) -> AgentSource {
        self.source
    }
    fn file_path(&self) -> &Path {
        &self.file_path
    }
    fn target_name(&self) -> &str {
        &self.name
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
    // pi `agent-management.ts:624-627` @ v0.43.0, ahead of every other branch: a match set spanning
    // several DISTINCT canonical names means the requested string was an ambiguous alias (or an
    // ambiguous name), and a mutating action must refuse outright rather than silently mutate one of
    // them. Names are listed sorted, de-duplicated.
    let distinct: Vec<String> = matches
        .iter()
        .map(|m| m.target_name().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if distinct.len() > 1 {
        return Err(ManagementOutcome::err(format!(
            "Ambiguous {} alias or name '{}': {}",
            kind.low(),
            name,
            distinct.join(", ")
        )));
    }
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
    // pi `agent-management.ts:672` @ v0.43.0: `if (agent.aliases?.length) lines.push(...)` — between
    // the package block and the model line.
    if !a.aliases.is_empty() {
        lines.push(format!("Aliases: {}", a.aliases.join(", ")));
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

/// pi `handleList` (`agent-management.ts:753-788` @v0.43.0 — thirty-six lines).
///
/// The proactive-skill block is spliced in exactly where upstream splices it: BETWEEN the `Chains:`
/// block and the chain diagnostics, preceded by one blank line and only when it has lines
/// (`agent-management.ts:784`'s
/// `...(proactiveSuggestions.length ? ["", ...proactiveSuggestions] : [])`). Its two inputs — pi's
/// `ctx.config?.proactiveSkillSubagents` and the result of its `discoverAvailableSkills(ctx.cwd)`
/// closure — arrive on [`ManagementRequest::proactive_skills`]; see [`ProactiveSkillsInput`] for
/// why the availability scan is pre-resolved by the async caller rather than run lazily here.
///
/// The recommender consults the SAME `agents`/`chains` bindings this function already rendered
/// (upstream passes its own post-filter `agents` and `chains` locals), so a scope-filtered or
/// disabled-filtered listing recommends only from what it listed.
///
/// There is no companion-suggestion block to port: upstream
/// deleted `companionSuggestionLines` from `handleList`'s `ManagementContext` and from its rendered
/// lines in `3ac0ef5` ("Make supervisor coordination native", 2026-07-03), together with the whole
/// `extension/companion-suggestions.ts` module.
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
            // pi `agent-management.ts:774` @ v0.43.0 appends `, aliases: <a, b>` after the optional
            // context segment and before the `: <description>` separator.
            let aliases = if a.aliases.is_empty() {
                String::new()
            } else {
                format!(", aliases: {}", a.aliases.join(", "))
            };
            lines.push(format!(
                "- {} ({}{}{}): {}",
                a.name,
                source_str(a.source),
                ctx,
                aliases,
                a.description
            ));
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
    // pi `agent-management.ts:765-770,784`: the proactive suggestions are computed from the same
    // filtered `agents`/`chains` this listing rendered, and spliced in after `Chains:` and before
    // `Chain diagnostics:` — with a leading blank line, and only when non-empty.
    if let Some(proactive) = &req.proactive_skills {
        let agent_inputs: Vec<crate::discovery::skills::ProactiveAgentInput> = agents
            .iter()
            .map(|a| crate::discovery::skills::proactive_agent_input(a))
            .collect();
        let chain_inputs: Vec<crate::discovery::skills::ProactiveChainInput> = chains
            .iter()
            .map(|c| crate::discovery::skills::proactive_chain_input(c))
            .collect();
        let suggestions =
            crate::discovery::skills::build_proactive_skill_subagent_recommendation_lines(
                &agent_inputs,
                &chain_inputs,
                proactive.setting,
                // The availability scan already happened (see `ProactiveSkillsInput`); this closure
                // is the sync shim that hands its result to the recommender, and — exactly like
                // upstream's — is never called when the feature is disabled.
                || Ok::<_, std::convert::Infallible>(proactive.available_skills.to_vec()),
            );
        if !suggestions.is_empty() {
            lines.push(String::new());
            lines.extend(suggestions);
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
        // pi `handleGet` @ v0.43.0 (`agent-management.ts:871-885`) checks AMBIGUITY first: a match
        // set spanning several distinct canonical names is refused before the not-found branch.
        let distinct = distinct_agent_names(&matches);
        if distinct.len() > 1 {
            let msg = format!(
                "Ambiguous agent alias or name '{}': {}",
                agent_name,
                distinct.join(", ")
            );
            if !has_both {
                return Ok(ManagementOutcome::err(msg));
            }
            blocks.push(msg);
        } else if matches.is_empty() {
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

/// Port of pi `formatModelSource` (`agent-management.ts:568-578`). The live parent session model is
/// now threaded in as `current_session_model` (from [`ManagementRequest::current_session_model`] /
/// [`cyrup_ext::host::HostServices::current_model`]), so when the persona declares no `model` but a
/// live session model is bound this reports "inherits current session model" (pi's own wording,
/// agent-management.ts:576); otherwise it classifies from discovery-time provenance (`override_info`
/// / `model_source`) and the agent's own resolved `model`.
fn format_model_source(agent: &AgentDefinition, current_session_model: Option<&str>) -> String {
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
    if current_session_model.is_some() {
        return "inherits current session model".to_string();
    }
    "inherit requested, but no current session model is available".to_string()
}

/// pi `handleModels` (`agent-management.ts:580-647`): the live parent session model is now threaded
/// in via [`ManagementRequest::current_session_model`] (from
/// [`cyrup_ext::host::HostServices::current_model`]), so `Current session model` renders the real
/// `provider/id` and an inheriting persona's effective model falls back to it; both degrade to
/// `(unavailable)`/`(unresolved)` only when there is genuinely no live session (headless /
/// SDK-embedder). The requested-filter validation, override provenance, and disabled state are
/// faithful. NB: the live `/subagents-models` slash + `subagent` tool `models` action route through
/// [`crate::extension::SubagentExecutor::run_models_report`] (which has its own `HostServices`
/// handle); this handler is the management-layer twin, reached via
/// [`handle_management_action`] and this crate's tests.
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
            .or_else(|| req.current_session_model.map(str::to_string))
            .unwrap_or_else(|| "(unresolved)".to_string());
        let mut lines = vec![
            "Builtin subagent model".to_string(),
            String::new(),
            format!("Agent: {name}"),
            "Effective model:".to_string(),
            format!("  {resolved}"),
            format!("Source: {}", format_model_source(agent, req.current_session_model)),
        ];
        if let Some(info) = &agent.override_info {
            lines.push("Override file:".to_string());
            lines.push(format!("  {}", info.settings_path.display()));
        }
        if agent.disabled == Some(true) {
            lines.push("Disabled: true".to_string());
        }
        lines.push("Current session model:".to_string());
        lines.push(format!("  {}", req.current_session_model.unwrap_or("(unavailable)")));
        return Ok(ManagementOutcome::ok(lines.join("\n")));
    }

    let mut lines = vec![
        "Builtin subagent models".to_string(),
        String::new(),
        "Current session model:".to_string(),
        format!("  {}", req.current_session_model.unwrap_or("(unavailable)")),
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
                    .or_else(|| req.current_session_model.map(str::to_string))
                    .unwrap_or_else(|| "(unresolved)".to_string());
                let source = format!(
                    "{}{}",
                    format_model_source(agent, req.current_session_model),
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
    // On CREATE the target's name is the just-built runtime name (pi `agent-management.ts:953-965`
    // constructs the `AgentConfig` with `name: runtimeName` before calling `applyAgentConfig`).
    if let Err(e) = apply_agent_config(&mut fields, &cfg_map, &runtime_name) {
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
    let mut base = match &target.override_info {
        Some(info) => (*info.base_snapshot).clone(),
        None => target.clone(),
    };
    // pi `editableAgentConfig` (`agent-management.ts:243`):
    // `...(agent.extensionsFromDefault ? {} : agent.extensions !== undefined ? { extensions: [...] } : {})`
    // — an `extensions` list that came from `subagents.defaultExtensions` is NOT the agent's own
    // data, so it is dropped here rather than BAKED into the `.md` file by the next update. (cyrup's
    // `base_snapshot` is a whole-definition clone, unlike pi's field-subset `cloneOverrideBase`, so
    // this guard also covers pi's `agents.ts:582` exclusion on the override-restore baseline.)
    if base.extensions_from_default {
        base.extensions = None;
        base.extensions_from_default = false;
    }
    base
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
        if let Err(e) = apply_agent_config(&mut fields, &cfg_map, &old_name) {
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

// =================================================================================================
// SUBA-005: the tier-aware / settings-writing management actions
// (pi `handleEject`/`handleDisable`/`handleEnable`/`handleReset`, `agent-management.ts:909-1032`)
// =================================================================================================
//
// These four are the last of pi's ten `ManagementAction`s to be ported. They differ from the six
// CRUD actions above in two ways that shape everything below:
//
//  1. **They are tier-aware.** `eject`/`reset` must see the *bundled* (builtin/package) source file
//     even when a same-named user/project file shadows it out of the R-SA-001 merge, and must
//     separately see that shadowing file. `discover_agents_all` returns only the merge winner, so
//     these two read [`crate::discovery::scan_agent_tiers`] (the raw, unmerged four-tier scan —
//     pi's `d.builtin`/`d.package`/`d.user`/`d.project`) *in addition to* the merged view they use
//     for name/chain-collision checks and the "Available: …" listing.
//  2. **`disable`/`enable`/`reset` WRITE `settings.json`,** via
//     [`crate::discovery::settings_write`]. They are the only management actions that mutate
//     anything other than an agent `.md` file.
//
// pi's distinguishing behavior — faithfully reproduced — is that `disable`/`enable` do not trust
// the write: they RE-RUN discovery afterwards and report an error if the agent's effective state did
// not actually change, naming the higher-precedence scope that is winning. A settings write that is
// silently overruled by the other scope is exactly the failure a user cannot debug on their own.

/// pi `actionScope` (`agent-management.ts:79-83`): unlike the CRUD actions' `asDisambiguationScope`
/// (where an absent/unrecognized `agentScope` means "both, disambiguate later"), these four actions
/// each write to exactly ONE scope, so an absent `agentScope` defaults to `user` and anything other
/// than `user`/`project` is a hard validation error naming the action.
fn action_scope(scope: Option<&str>, action: &str) -> Result<AgentSource, ManagementOutcome> {
    match scope {
        None => Ok(AgentSource::User),
        Some("user") => Ok(AgentSource::User),
        Some("project") => Ok(AgentSource::Project),
        _ => Err(ManagementOutcome::err(format!(
            "agentScope must be 'user' or 'project' for {action}."
        ))),
    }
}

/// pi `resolveEffectiveAgent` (`agent-management.ts:138-152` @ v0.43.0, renamed from
/// `pickEffectiveAgent` when it became alias-aware): the single highest-precedence agent answering
/// to `name` — verbatim, by alias, or (only when neither matched) after [`sanitize_name`].
///
/// Three outcomes, matching pi's `{ agent?, error? }`:
/// * `Ok(Some(agent))` — resolved.
/// * `Ok(None)` — nothing answers; the caller emits its own "not found. Available: …".
/// * `Err(message)` — the name/alias is AMBIGUOUS; the caller surfaces the message verbatim. This
///   outcome did not exist before aliases, and it must not be collapsed into `Ok(None)`: a
///   "not found" message for a name that matched two agents would be actively misleading.
///
/// The sanitized retry is gated on the first attempt being a clean MISS (pi's
/// `!resolved.agent && !resolved.error`) — an ambiguous raw name is never retried.
///
/// pi reduces over its concatenated per-tier arrays by `AGENT_SOURCE_PRECEDENCE`; cyrup's
/// `discover_agents_all` has *already* performed that reduction per name (R-SA-001), so the reduce
/// below normally sees a single element — it is kept so the precedence rule is stated, not implied.
fn resolve_effective_agent(
    d: &AgentDiscoveryResult,
    name: &str,
) -> Result<Option<AgentDefinition>, String> {
    let raw = name.trim();
    let mut resolved = resolve_agent_name(raw, &d.agents);
    if matches!(resolved, AgentNameResolution::NotFound) {
        let sanitized = sanitize_name(raw);
        if sanitized != raw {
            resolved = resolve_agent_name(&sanitized, &d.agents);
        }
    }
    if let Some(err) = resolved.error() {
        return Err(err.to_string());
    }
    let Some(agent) = resolved.agent() else {
        return Ok(None);
    };
    let canonical = agent.name.clone();
    Ok(d.agents
        .iter()
        .filter(|a| a.name == canonical)
        .min_by_key(|a| a.source.precedence_rank())
        .cloned())
}

/// The bundled (read-only) tiers in pi's own `[...d.package, ...d.builtin]` search order
/// (`agent-management.ts:917`, `:1005`) — package first, so a package agent shadowing a same-named
/// builtin is the one `eject`/`reset` treat as "the bundled default", matching R-SA-001's
/// Package-beats-Builtin precedence.
fn find_bundled<'a>(
    tiers: &'a super::merge::TieredAgents,
    raw: &str,
    sanitized: &str,
) -> Option<&'a AgentDefinition> {
    tiers
        .package
        .iter()
        .chain(tiers.builtin.iter())
        .find(|a| a.name == raw || a.name == sanitized)
}

/// The raw (unmerged) writable tier for `scope` — pi's `scope === "user" ? d.user : d.project`.
fn writable_tier(tiers: &super::merge::TieredAgents, scope: AgentSource) -> &[AgentDefinition] {
    match scope {
        AgentSource::Project => &tiers.project,
        _ => &tiers.user,
    }
}

/// The `settings.json` path these actions write for `scope`, or pi's verbatim refusal when the
/// project scope does not exist at all (`agent-management.ts:955-957`, mirrored for enable/reset).
///
/// `project_settings_path` is `None` **only** when the discovery config was built with no project
/// root; an existing project root whose `settings.json` has not been created yet is `Some(path)`
/// (the writers below `mkdir -p` and create it). pi's `.pi or .agents` wording is rebranded to
/// cyrup's own config-directory names, matching this crate's standing `.pi` -> `.cyrup` rename.
fn scope_settings_path(
    cfg: &AgentDiscoveryConfig,
    scope: AgentSource,
) -> Result<PathBuf, ManagementOutcome> {
    match scope {
        AgentSource::Project => cfg.override_settings.project_settings_path.clone().ok_or_else(|| {
            ManagementOutcome::err(
                "Project override is not available here: no project config root (.cyrup or .agents) \
                 was found above the cwd. Use agentScope: 'user' or run from inside a project.",
            )
        }),
        _ => Ok(cfg.override_settings.user_settings_path.clone()),
    }
}

/// Re-read BOTH `settings.json` files from disk into a fresh copy of `cfg`.
///
/// **Required after any settings write, and easy to get wrong.** pi's post-write verification calls
/// `discoverAgentsAll(ctx.cwd)`, which re-reads the settings files as part of discovery. cyrup's
/// [`discover_agents_all`] instead consumes the already-parsed
/// [`AgentDiscoveryConfig::override_settings`] snapshot its caller loaded — so re-running it with
/// the SAME `cfg` after a write re-applies the PRE-write settings and reports the exact opposite of
/// what happened (a successful disable reads back as "still enabled", a successful enable as "still
/// disabled"). Every `disable`/`enable` verification below goes through this function.
fn with_settings_reread(cfg: &AgentDiscoveryConfig) -> Result<AgentDiscoveryConfig, SubagentError> {
    let mut refreshed = cfg.clone();
    refreshed.override_settings = super::load_layered_override_settings(
        &cfg.override_settings.user_settings_path,
        cfg.override_settings.project_settings_path.as_deref(),
    )?;
    Ok(refreshed)
}

/// pi `handleEject` (`agent-management.ts:909-943`): copy a read-only builtin/package agent file
/// verbatim into a writable scope so it can be customized, refusing rather than clobbering whenever
/// the destination is already occupied.
///
/// Deliberately a **byte-for-byte file copy**, not a re-serialization of the parsed
/// [`AgentDefinition`]: an ejected file must be the bundled author's original text (comments,
/// field order, prose formatting and any frontmatter key this crate's parser ignores all survive),
/// which round-tripping through [`write_agent_file`] would not preserve.
fn handle_eject(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    let Some(agent_param) = req.agent else {
        return Ok(ManagementOutcome::err("Specify 'agent' for eject."));
    };
    let raw = agent_param.trim();
    let sanitized = sanitize_name(raw);
    let scope = match action_scope(req.agent_scope, "eject") {
        Ok(s) => s,
        Err(outcome) => return Ok(outcome),
    };

    let d = discover_agents_all(cfg)?;
    let tiers = super::scan_agent_tiers(cfg);
    let Some(source) = find_bundled(&tiers, raw, &sanitized) else {
        let avail = available_agent_names(&d);
        return Ok(ManagementOutcome::err(format!(
            "Agent '{raw}' not found or is not a bundled/package agent. eject copies a builtin or \
             package agent to {} scope so it can be customized. Available: {}.",
            source_str(scope),
            if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
        )));
    };
    let runtime_name = source.name.clone();

    if let Some(existing) = writable_tier(&tiers, scope).iter().find(|a| a.name == runtime_name) {
        return Ok(ManagementOutcome::err(format!(
            "Agent '{runtime_name}' is already a custom {} agent at {}. Edit it with {{ action: \"update\", agent: \"{runtime_name}\" }} or delete it first.",
            source_str(scope),
            existing.file_path.display()
        )));
    }
    // The remaining collision pi's `nameExistsInScope` guards against is a same-named CHAIN (the
    // same-named-agent case is already answered above, from the raw tier rather than the merge).
    if name_exists_in_scope(&d, scope, &runtime_name, None) {
        return Ok(ManagementOutcome::err(format!(
            "An agent or chain named '{runtime_name}' already exists in {} scope. Remove or rename it first.",
            source_str(scope)
        )));
    }

    let Some(target_dir) = pick_scope_dir(cfg, scope, false) else {
        return Ok(ManagementOutcome::err(format!(
            "No {} agents directory is configured to eject into.",
            source_str(scope)
        )));
    };
    std::fs::create_dir_all(&target_dir).map_err(SubagentError::Spawn)?;
    let target_path = agent_file_path(&target_dir, &runtime_name);
    if target_path.exists() {
        // Reachable only when the destination holds a file discovery REFUSED to parse as an agent
        // (missing `name`/`description`, R-SA-005) — otherwise the tier check above would have
        // fired. Refuse rather than overwrite: the file is someone's, whatever it is.
        return Ok(ManagementOutcome::err(format!(
            "File already exists at {} but is not a valid agent definition. Remove or rename it first.",
            target_path.display()
        )));
    }
    let content = match std::fs::read_to_string(&source.file_path) {
        Ok(content) => content,
        Err(e) => {
            return Ok(ManagementOutcome::err(format!(
                "Failed to read source agent at {}: {e}",
                source.file_path.display()
            )));
        }
    };
    std::fs::write(&target_path, content).map_err(SubagentError::Spawn)?;
    Ok(ManagementOutcome::ok(format!(
        "Ejected agent '{runtime_name}' from {} to {} scope at {}. Edit it there to customize; it \
         shadows the bundled {} agent of the same name.",
        source_str(source.source),
        source_str(scope),
        target_path.display(),
        source_str(source.source)
    )))
}

/// pi `handleDisable` (`agent-management.ts:947-968`): write `{ disabled: true }` into
/// `subagents.agentOverrides.<name>` at `scope`, then RE-DISCOVER and verify the agent actually
/// became invisible — reporting an error (naming the winning scope) if a higher-precedence override
/// overruled the write.
fn handle_disable(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    let Some(agent_param) = req.agent else {
        return Ok(ManagementOutcome::err("Specify 'agent' for disable."));
    };
    let raw = agent_param.trim();
    let scope = match action_scope(req.agent_scope, "disable") {
        Ok(s) => s,
        Err(outcome) => return Ok(outcome),
    };
    let settings_path = match scope_settings_path(cfg, scope) {
        Ok(p) => p,
        Err(outcome) => return Ok(outcome),
    };

    let d = discover_agents_all(cfg)?;
    // pi `agent-management.ts:987-988` @ v0.43.0: the AMBIGUITY outcome is surfaced verbatim and
    // short-circuits ahead of the not-found message.
    let effective = match resolve_effective_agent(&d, raw) {
        Err(msg) => return Ok(ManagementOutcome::err(msg)),
        Ok(Some(agent)) => agent,
        Ok(None) => {
            let avail = available_agent_names(&d);
            return Ok(ManagementOutcome::err(format!(
                "Agent '{raw}' not found. Available: {}.",
                if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
            )));
        }
    };
    let runtime_name = effective.name;

    let mut fields = serde_json::Map::new();
    fields.insert("disabled".to_string(), serde_json::Value::Bool(true));
    super::settings_write::merge_builtin_agent_override(&settings_path, &runtime_name, &fields)?;

    // pi re-runs `discoverAgentsAll` and inspects the effective agent again: the write is only a
    // success if the agent is ACTUALLY disabled now. `discover_agents_all` is the management view,
    // which by R-SA-013 still lists disabled agents — so a disabled agent is found here with
    // `disabled: Some(true)`, which is precisely the signal being checked. The re-read
    // ([`with_settings_reread`]) is what makes this a verification rather than a replay of the
    // pre-write snapshot.
    let after =
        resolve_effective_agent(&discover_agents_all(&with_settings_reread(cfg)?)?, raw).ok().flatten();
    if after.as_ref().and_then(|a| a.disabled) == Some(true) {
        return Ok(ManagementOutcome::ok(format!(
            "Disabled agent '{runtime_name}' via {} settings override at {}. It is now hidden from \
             runtime discovery and {{ action: \"list\" }}.",
            source_str(scope),
            settings_path.display()
        )));
    }
    let winning = after
        .as_ref()
        .and_then(|a| a.override_info.as_ref())
        .map_or("project", |o| override_scope_str(o.scope));
    Ok(ManagementOutcome::err(format!(
        "Wrote a disabled override for '{runtime_name}' at {}, but the agent is still enabled. A \
         higher-precedence {winning} override is likely winning. Try agentScope: '{winning}'.",
        settings_path.display()
    )))
}

/// pi `handleEnable` (`agent-management.ts:970-996`): remove ONLY the `disabled` field from
/// `subagents.agentOverrides.<name>` at `scope` (an agent's other overrides — its model, tools,
/// thinking budget — survive being re-enabled), then re-discover and verify.
fn handle_enable(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    let Some(agent_param) = req.agent else {
        return Ok(ManagementOutcome::err("Specify 'agent' for enable."));
    };
    let raw = agent_param.trim();
    let scope = match action_scope(req.agent_scope, "enable") {
        Ok(s) => s,
        Err(outcome) => return Ok(outcome),
    };
    let settings_path = match scope_settings_path(cfg, scope) {
        Ok(p) => p,
        Err(outcome) => return Ok(outcome),
    };

    let d = discover_agents_all(cfg)?;
    // pi `agent-management.ts:987-988` @ v0.43.0: the AMBIGUITY outcome is surfaced verbatim and
    // short-circuits ahead of the not-found message.
    let effective = match resolve_effective_agent(&d, raw) {
        Err(msg) => return Ok(ManagementOutcome::err(msg)),
        Ok(Some(agent)) => agent,
        Ok(None) => {
            let avail = available_agent_names(&d);
            return Ok(ManagementOutcome::err(format!(
                "Agent '{raw}' not found. Available: {}.",
                if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
            )));
        }
    };
    let runtime_name = effective.name;

    let removed = super::settings_write::remove_builtin_agent_override_fields(
        &settings_path,
        &runtime_name,
        &["disabled"],
    )?;
    // Re-read from disk before verifying — see [`with_settings_reread`].
    let after =
        resolve_effective_agent(&discover_agents_all(&with_settings_reread(cfg)?)?, raw).ok().flatten();

    if let Some(after) = after.as_ref()
        && after.disabled != Some(true)
    {
        return Ok(ManagementOutcome::ok(if removed {
            format!(
                "Enabled agent '{runtime_name}' (removed disabled override at {}).",
                settings_path.display()
            )
        } else {
            format!("Agent '{runtime_name}' is already enabled.")
        }));
    }
    if let Some(info) = after.as_ref().and_then(|a| a.override_info.as_ref())
        && override_scope_str(info.scope) != source_str(scope)
    {
        return Ok(ManagementOutcome::err(format!(
            "Agent '{runtime_name}' is still disabled via a {} scope override at {}. Specify \
             agentScope: '{}' to enable it.",
            override_scope_str(info.scope),
            info.settings_path.display(),
            override_scope_str(info.scope)
        )));
    }
    let (hint_scope, hint_path) = after
        .as_ref()
        .and_then(|a| a.override_info.as_ref())
        .map_or_else(
            || (source_str(scope).to_string(), settings_path.display().to_string()),
            |o| (override_scope_str(o.scope).to_string(), o.settings_path.display().to_string()),
        );
    Ok(ManagementOutcome::err(format!(
        "Agent '{runtime_name}' is still disabled after removing the {} disabled override. It may \
         be hidden via subagents.disableBuiltins in {hint_scope} settings at {hint_path}.",
        source_str(scope)
    )))
}

/// pi `handleReset` (`agent-management.ts:998-1032`): undo BOTH halves of a customization at
/// `scope` — delete the custom `.md` file that shadows a bundled agent, and delete the whole
/// `subagents.agentOverrides.<name>` entry — returning the agent to its bundled default.
///
/// Distinct from `delete` (which removes a custom agent that has no bundled default and leaves
/// settings alone) and from `enable` (which removes only the `disabled` field). Reset with nothing
/// to reset is a **success**, not an error, and says so.
fn handle_reset(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError> {
    let Some(agent_param) = req.agent else {
        return Ok(ManagementOutcome::err("Specify 'agent' for reset."));
    };
    let raw = agent_param.trim();
    let sanitized = sanitize_name(raw);
    let scope = match action_scope(req.agent_scope, "reset") {
        Ok(s) => s,
        Err(outcome) => return Ok(outcome),
    };
    let settings_path = match scope_settings_path(cfg, scope) {
        Ok(p) => p,
        Err(outcome) => return Ok(outcome),
    };

    let d = discover_agents_all(cfg)?;
    let tiers = super::scan_agent_tiers(cfg);
    let Some(bundled) = find_bundled(&tiers, raw, &sanitized) else {
        let custom = tiers
            .user
            .iter()
            .chain(tiers.project.iter())
            .find(|a| a.name == raw || a.name == sanitized);
        if let Some(custom) = custom {
            return Ok(ManagementOutcome::err(format!(
                "Agent '{raw}' has no bundled default to reset to. Use {{ action: \"delete\", agent: \"{}\" }} to remove the custom {} agent.",
                custom.name,
                source_str(custom.source)
            )));
        }
        let avail = available_agent_names(&d);
        return Ok(ManagementOutcome::err(format!(
            "Agent '{raw}' not found. Available: {}.",
            if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
        )));
    };
    let runtime_name = bundled.name.clone();
    let bundled_source = bundled.source;

    let mut lines: Vec<String> = Vec::new();
    if let Some(custom) = writable_tier(&tiers, scope)
        .iter()
        .find(|a| a.name == raw || a.name == sanitized)
    {
        std::fs::remove_file(&custom.file_path).map_err(SubagentError::Spawn)?;
        lines.push(format!(
            "Deleted custom {} agent file at {}.",
            source_str(scope),
            custom.file_path.display()
        ));
    }
    if super::settings_write::remove_builtin_agent_override(&settings_path, &runtime_name)? {
        lines.push(format!(
            "Removed {} settings override at {}.",
            source_str(scope),
            settings_path.display()
        ));
    }

    if lines.is_empty() {
        let other_scope = match scope {
            AgentSource::Project => AgentSource::User,
            _ => AgentSource::Project,
        };
        let other_custom = writable_tier(&tiers, other_scope)
            .iter()
            .any(|a| a.name == raw || a.name == sanitized);
        // pi reads `bundled.override?.scope` off its per-tier (override-applied) builtin entry;
        // cyrup applies overrides during the merge, so the equivalent provenance lives on the merged
        // winner for this name — which, in this branch (no customization at `scope`), is the bundled
        // agent itself unless the OTHER scope shadows it, exactly the case this hint is about.
        let has_other_override = d
            .agents
            .iter()
            .find(|a| a.name == runtime_name)
            .and_then(|a| a.override_info.as_ref())
            .is_some_and(|o| override_scope_str(o.scope) == source_str(other_scope));
        let note = if other_custom || has_other_override {
            format!(
                " Customization exists in {0} scope; specify agentScope: '{0}' to reset it.",
                source_str(other_scope)
            )
        } else {
            String::new()
        };
        return Ok(ManagementOutcome::ok(format!(
            "Agent '{runtime_name}' has no {} customization to reset.{note} It is at its bundled {} default.",
            source_str(scope),
            source_str(bundled_source)
        )));
    }
    lines.push(format!(
        "Reset agent '{runtime_name}' to its bundled {} default.",
        source_str(bundled_source)
    ));
    Ok(ManagementOutcome::ok(lines.join("\n")))
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
            aliases: Vec::new(),
            tools: None,
            extensions: None,
            extensions_from_default: false,
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
            default_async: None,
            default_timeout_ms: None,
            memory: None,
            tool_budget: None,
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

    /// A `memory:`/`toolBudget:` agent must survive a serialize -> re-parse round-trip. This is
    /// the failure mode that adding a key to `KNOWN_FIELDS` creates: the extra-fields loop skips
    /// known keys, so a known key the serializer never EMITS is silently deleted the first time a
    /// management update or rename rewrites the file.
    #[test]
    fn serialize_agent_round_trips_memory_and_tool_budget() {
        use crate::discovery::frontmatter::parse_agent_file;

        let mut def = sample_agent(AgentSource::Project, PathBuf::from("/w.md"));
        def.local_name = "reviewer".to_string();
        def.name = "reviewer".to_string();
        def.description = "Reviews".to_string();
        def.system_prompt_body = "Do work".to_string();
        def.memory = Some(crate::discovery::types::AgentMemoryConfig {
            scope: crate::discovery::types::MemoryScope::Project,
            path: "security-reviewer".to_string(),
        });
        def.tool_budget = crate::exec::tool_budget::validate_tool_budget_config(
            Some(&serde_json::json!({ "hard": 8, "soft": 3, "block": ["read"] })),
            "toolBudget",
        )
        .expect("valid");

        let serialized = serialize_agent(&def, None);
        assert!(
            serialized.contains("memory:\n  scope: project\n  path: security-reviewer"),
            "memory must be emitted as a nested block:\n{serialized}"
        );
        assert!(
            serialized.contains("toolBudget: {"),
            "toolBudget must be emitted as JSON:\n{serialized}"
        );

        let reparsed = parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))
            .expect("round-trips back through the parser");
        assert_eq!(reparsed.memory, def.memory, "memory lost on round trip");
        assert_eq!(
            reparsed.tool_budget, def.tool_budget,
            "toolBudget lost on round trip"
        );
        assert!(!reparsed.extra_fields.contains_key("memory"));
        assert!(!reparsed.extra_fields.contains_key("toolBudget"));
    }

    /// The same silent-deletion trap for the two launch defaults (G98).
    #[test]
    fn serialize_agent_round_trips_the_async_and_timeout_launch_defaults() {
        use crate::discovery::frontmatter::parse_agent_file;

        let mut def = sample_agent(AgentSource::Project, PathBuf::from("/w.md"));
        def.local_name = "slowpoke".to_string();
        def.name = "slowpoke".to_string();
        def.description = "Slow".to_string();
        def.system_prompt_body = "Do work".to_string();
        def.default_async = Some(true);
        def.default_timeout_ms = Some(1234);

        let serialized = serialize_agent(&def, None);
        assert!(serialized.contains("async: true"), "{serialized}");
        assert!(serialized.contains("timeoutMs: 1234"), "{serialized}");

        let reparsed = parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))
            .expect("round-trips");
        assert_eq!(reparsed.default_async, Some(true));
        assert_eq!(reparsed.default_timeout_ms, Some(1234));
        assert!(!reparsed.extra_fields.contains_key("async"));
        assert!(!reparsed.extra_fields.contains_key("timeoutMs"));

        // An explicit `async: false` must survive too (it is not the same as "unset").
        def.default_async = Some(false);
        let serialized = serialize_agent(&def, None);
        let reparsed = parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))
            .expect("round-trips");
        assert_eq!(reparsed.default_async, Some(false));
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
        ManagementRequest {
            agent,
            chain_name: chain,
            agent_scope: scope,
            config,
            current_session_model: None,
            proactive_skills: None,
        }
    }

    // ---- pi `handleList`'s proactive skill-subagent block (`agent-management.ts:765-770,784`) ----

    /// Two user agents that both name the same skill, so the skill clears the default
    /// `minReferences: 2`. Returns the request-side availability list that makes it recommendable.
    fn seed_two_agents_sharing_a_skill(cfg: &AgentDiscoveryConfig) -> Vec<crate::discovery::skills::AvailableSkill> {
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "auditor-one.md",
            "---\nname: auditor-one\ndescription: First auditor\nskills: audit-trail\n---\nBody.\n",
        );
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "auditor-two.md",
            "---\nname: auditor-two\ndescription: Second auditor\nskills: audit-trail\n---\nBody.\n",
        );
        vec![crate::discovery::skills::AvailableSkill {
            name: "audit-trail".to_string(),
            description: Some("Trace every mutation.".to_string()),
        }]
    }

    /// The block upstream splices at `agent-management.ts:784` must actually appear in `list`
    /// output, positioned AFTER the `Chains:` block and BEFORE `Chain diagnostics:`, with the
    /// blank-line separator upstream's `["", ...proactiveSuggestions]` prepends.
    #[test]
    fn list_emits_the_proactive_skill_subagent_block_in_pis_position() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let available = seed_two_agents_sharing_a_skill(&cfg);

        let mut req = mreq(None, None, None, None);
        req.proactive_skills = Some(ProactiveSkillsInput {
            setting: None, // pi's `undefined` — defaults on
            available_skills: &available,
        });
        let out = handle_management_action(&cfg, "list", &req).expect("list ok");
        assert!(!out.is_error, "{}", out.text);
        let t = out.text;

        assert!(
            t.contains("Proactive skill subagent suggestions:"),
            "the block upstream splices at `agent-management.ts:784` is missing:\n{t}"
        );
        assert!(
            t.contains("- audit-trail via reviewer (referenced by 2 configured agents/chains; agent:auditor-one, agent:auditor-two) - Trace every mutation."),
            "the recommendation line must match `formatProactiveSkillSubagentRecommendations`:\n{t}"
        );
        assert!(
            t.contains("Guardrails: use these for broad tasks"),
            "the guardrails footer must ship with the block:\n{t}"
        );
        let chains_at = t.find("Chains:").unwrap_or(usize::MAX);
        let block_at = t.find("Proactive skill subagent suggestions:").unwrap_or(usize::MIN);
        assert!(chains_at < block_at, "the block must follow `Chains:`:\n{t}");
        assert!(
            t.contains("\n\nProactive skill subagent suggestions:"),
            "upstream prepends one blank line to the block:\n{t}"
        );
    }

    /// pi reads `ctx.config?.proactiveSkillSubagents`; the literal `false` disables the feature
    /// entirely (`resolveProactiveSkillSubagentsConfig`, `proactive-skills.ts:38-59`). A setting
    /// that stopped being threaded through would silently stop disabling anything.
    #[test]
    fn list_honours_an_explicit_proactive_skill_subagents_false() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let available = seed_two_agents_sharing_a_skill(&cfg);

        let disabled = crate::discovery::skills::ProactiveSkillSubagentsSetting::Disabled;
        let mut req = mreq(None, None, None, None);
        req.proactive_skills = Some(ProactiveSkillsInput {
            setting: Some(&disabled),
            available_skills: &available,
        });
        let out = handle_management_action(&cfg, "list", &req).expect("list ok");
        assert!(
            !out.text.contains("Proactive skill subagent suggestions:"),
            "an explicit `false` must suppress the block:\n{}",
            out.text
        );
    }

    /// A caller that ran no availability scan (`proactive_skills: None`) emits no block — the same
    /// outcome upstream reaches when its `discoverAvailableSkills` throws
    /// (`proactive-skills.ts:182-186` catches to `[]`, which matches no skill).
    #[test]
    fn list_emits_no_proactive_block_without_an_availability_scan() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let _available = seed_two_agents_sharing_a_skill(&cfg);

        let out = handle_management_action(&cfg, "list", &mreq(None, None, None, None)).expect("list ok");
        assert!(
            !out.text.contains("Proactive skill subagent suggestions:"),
            "{}",
            out.text
        );
        // ...and an availability scan that found nothing likewise recommends nothing.
        let empty: Vec<crate::discovery::skills::AvailableSkill> = Vec::new();
        let mut req = mreq(None, None, None, None);
        req.proactive_skills = Some(ProactiveSkillsInput {
            setting: None,
            available_skills: &empty,
        });
        let out = handle_management_action(&cfg, "list", &req).expect("list ok");
        assert!(
            !out.text.contains("Proactive skill subagent suggestions:"),
            "{}",
            out.text
        );
    }

    /// The extension-config shape (`config.json`'s `proactiveSkillSubagents`) must reach the
    /// recommender's own setting shape without losing the disable — the bridge is what
    /// `extension.rs::route_management_action` calls.
    #[test]
    fn the_extension_config_bridge_preserves_disable_and_the_tuning_knobs() {
        use crate::discovery::skills::{
            ProactiveSkillSubagentsSetting, resolve_proactive_skill_subagents_config,
        };
        use crate::registration::ProactiveSkillSubagents;

        let off = ProactiveSkillSubagentsSetting::from_extension_config(
            &ProactiveSkillSubagents::Toggle(false),
        );
        assert!(!resolve_proactive_skill_subagents_config(Some(&off)).enabled);

        let on = ProactiveSkillSubagentsSetting::from_extension_config(
            &ProactiveSkillSubagents::Toggle(true),
        );
        assert!(resolve_proactive_skill_subagents_config(Some(&on)).enabled);

        let tuned = ProactiveSkillSubagentsSetting::from_extension_config(
            &ProactiveSkillSubagents::Config(crate::registration::ProactiveSkillSubagentsConfig {
                enabled: Some(true),
                min_references: Some(1),
                max_recommendations: Some(2),
                preferred_agent: Some("scout".to_string()),
            }),
        );
        let resolved = resolve_proactive_skill_subagents_config(Some(&tuned));
        assert_eq!(resolved.min_references, 1);
        assert_eq!(resolved.max_recommendations, 2);
        assert_eq!(resolved.preferred_agent, "scout");
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
    fn models_lists_builtin_mapping_without_a_live_session_degrades_to_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "models", &mreq(None, None, None, None)).expect("models ok");
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Builtin subagent models"), "{}", out.text);
        for name in BUILTIN_AGENT_NAMES {
            assert!(out.text.contains(name), "missing builtin {name}: {}", out.text);
        }
        // (d) No live session model bound (`current_session_model: None`) ⇒ the genuine no-host
        // degrade, exactly as before this seam existed.
        assert!(out.text.contains("Current session model:\n  (unavailable)"), "{}", out.text);
    }

    #[test]
    fn models_renders_the_live_inherited_session_model_when_bound() {
        // With a live parent session model threaded in (pi `ctx.model`), the report shows the REAL
        // `provider/id` on the `Current session model` line, and an inheriting builtin (no own
        // `model`) falls back to it as its effective model / "inherits current session model" source.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let model = "together/zai-org/GLM-5.2";
        let req = ManagementRequest {
            agent: None,
            chain_name: None,
            agent_scope: None,
            config: None,
            current_session_model: Some(model),
            proactive_skills: None,
        };
        let out = handle_management_action(&cfg, "models", &req).expect("models ok");
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains(&format!("Current session model:\n  {model}")),
            "the live inherited model must render instead of (unavailable): {}",
            out.text
        );
        assert!(
            !out.text.contains("(unavailable)"),
            "no (unavailable) degrade when a live session model is bound: {}",
            out.text
        );
    }

    #[test]
    fn models_rejects_unknown_builtin_filter() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "models", &mreq(Some("not-a-builtin"), None, None, None)).expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("Builtin agent 'not-a-builtin' not found"), "{}", out.text);
    }

    // -----------------------------------------------------------------------------------------
    // G97 — aliases through the real management surface
    // -----------------------------------------------------------------------------------------

    fn write_agent_md(dir: &Path, file: &str, body: &str) {
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(dir.join(file), body).expect("write agent file");
    }

    /// `aliases:` must survive a serialize -> re-parse round-trip — the same silent-deletion trap
    /// `memory:`/`toolBudget:` had: both spellings are now `KNOWN_FIELDS`, so a key the serializer
    /// never emits is dropped the first time management rewrites the file.
    #[test]
    fn serialize_agent_round_trips_aliases() {
        use crate::discovery::frontmatter::parse_agent_file;

        let mut def = sample_agent(AgentSource::Project, PathBuf::from("/w.md"));
        def.local_name = "oracle".to_string();
        def.name = "oracle".to_string();
        def.aliases = vec!["advisor".to_string(), "seer".to_string()];

        let serialized = serialize_agent(&def, None);
        assert!(
            serialized.contains("aliases: advisor, seer"),
            "aliases must be emitted comma-joined:\n{serialized}"
        );
        let reparsed = parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))
            .expect("round-trips back through the parser");
        assert_eq!(reparsed.aliases, def.aliases, "aliases lost on round trip");
        assert!(!reparsed.extra_fields.contains_key("aliases"));
        assert!(!reparsed.extra_fields.contains_key("alias"));

        // An agent with no aliases emits no line at all on a CREATE (pi's `if (aliasesValue || ...)`).
        def.aliases.clear();
        assert!(!serialize_agent(&def, None).contains("aliases:"));
    }

    /// An UPDATE that does not mention `aliases` must not delete an existing `alias:`/`aliases:`
    /// line — pi's preserve set covers both spellings (`agent-serializer.ts:60`).
    #[test]
    fn an_unrelated_update_preserves_an_existing_alias_line() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\nalias: prophet\n---\n\nBody\n",
        );

        let config = serde_json::json!({ "description": "Sees further" });
        let out = handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&config)))
            .expect("update ok");
        assert!(!out.is_error, "{}", out.text);

        let written = std::fs::read_to_string(cfg.user_agent_dirs[0].join("seer.md")).expect("read");
        assert!(
            written.contains("aliases: prophet"),
            "an update that never mentioned aliases must not drop them:\n{written}"
        );
    }

    /// `config.aliases` sets / clears the list, and rejects a wrong-typed value with pi's message
    /// (`agent-management.ts:411-421`).
    #[test]
    fn config_aliases_sets_clears_and_validates() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\n---\n\nBody\n",
        );

        // String (CSV) form, with the agent's own name filtered out.
        let set = serde_json::json!({ "aliases": "prophet, seer , oracle-lite" });
        let out = handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&set)))
            .expect("update ok");
        assert!(!out.is_error, "{}", out.text);
        let written = std::fs::read_to_string(cfg.user_agent_dirs[0].join("seer.md")).expect("read");
        assert!(
            written.contains("aliases: prophet, oracle-lite"),
            "the agent's own name must be filtered out of its aliases:\n{written}"
        );

        // Array form, de-duplicated.
        let arr = serde_json::json!({ "aliases": ["prophet", "prophet", " diviner "] });
        handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&arr)))
            .expect("update ok");
        let written = std::fs::read_to_string(cfg.user_agent_dirs[0].join("seer.md")).expect("read");
        assert!(written.contains("aliases: prophet, diviner"), "{written}");

        // `false` clears. pi's serializer emits the line only when there IS a value or when the
        // preserve set still carries the key — and `preservedAgentFrontmatterFields` REMOVES both
        // spellings for an update that set `aliases` (`agent-management.ts:287`) — so a clear drops
        // the line entirely rather than writing an empty one.
        let clear = serde_json::json!({ "aliases": false });
        handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&clear)))
            .expect("update ok");
        let written = std::fs::read_to_string(cfg.user_agent_dirs[0].join("seer.md")).expect("read");
        assert!(!written.contains("aliases:"), "a cleared alias list writes no line:\n{written}");
        let reparsed = crate::discovery::frontmatter::parse_agent_file(
            &written,
            AgentSource::User,
            Path::new("/seer.md"),
        )
        .expect("reparses");
        assert!(reparsed.aliases.is_empty());

        // Wrong type -> pi's exact validation message.
        let bad = serde_json::json!({ "aliases": 7 });
        let out = handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&bad)))
            .expect("no discovery error");
        assert!(out.is_error);
        assert_eq!(
            out.text,
            "config.aliases must be a comma-separated string, string array, or false when provided."
        );
    }

    /// `list` renders `, aliases: …` and `get` renders an `Aliases:` line
    /// (`agent-management.ts:672,774`); `get` is also reachable BY the alias.
    #[test]
    fn list_and_get_render_aliases_and_get_resolves_by_alias() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\naliases: prophet, diviner\n---\n\nBody\n",
        );

        let list = handle_management_action(&cfg, "list", &mreq(None, None, None, None)).expect("list ok");
        assert!(
            list.text.contains("- seer (user, aliases: prophet, diviner): Sees"),
            "{}",
            list.text
        );

        let by_alias = handle_management_action(&cfg, "get", &mreq(Some("prophet"), None, None, None))
            .expect("get ok");
        assert!(!by_alias.is_error, "{}", by_alias.text);
        assert!(by_alias.text.contains("Agent: seer (user)"), "{}", by_alias.text);
        assert!(by_alias.text.contains("Aliases: prophet, diviner"), "{}", by_alias.text);
    }

    /// Two agents claiming the SAME alias make every management path that would have to pick one
    /// refuse, with pi's `Ambiguous agent alias or name` wording (`agent-management.ts:624-626,880-882`).
    #[test]
    fn an_ambiguous_alias_is_refused_by_get_update_and_disable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\naliases: prophet\n---\n\nBody\n",
        );
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "augur.md",
            "---\nname: augur\ndescription: Augurs\naliases: prophet\n---\n\nBody\n",
        );

        let get = handle_management_action(&cfg, "get", &mreq(Some("prophet"), None, None, None))
            .expect("no discovery error");
        assert!(get.is_error);
        assert_eq!(get.text, "Ambiguous agent alias or name 'prophet': augur, seer");

        let config = serde_json::json!({ "description": "changed" });
        let update =
            handle_management_action(&cfg, "update", &mreq(Some("prophet"), None, None, Some(&config)))
                .expect("no discovery error");
        assert!(update.is_error);
        assert_eq!(update.text, "Ambiguous agent alias or name 'prophet': augur, seer");

        // `disable` goes through `resolve_effective_agent`, whose ambiguity message is
        // `resolveAgentName`'s own (`agents.ts:526`), surfaced verbatim.
        let disable =
            handle_management_action(&cfg, "disable", &mreq(Some("prophet"), None, Some("user"), None))
                .expect("no discovery error");
        assert!(disable.is_error);
        assert_eq!(disable.text, "Ambiguous agent alias 'prophet': augur, seer");
        assert!(
            !disable.text.contains("not found"),
            "an ambiguous alias must NEVER be reported as not found: {}",
            disable.text
        );
    }

    /// `disable`/`enable` reach their target BY alias and write the override under the agent's
    /// CANONICAL name (`agent-management.ts:987-991`).
    #[test]
    fn disable_by_alias_writes_the_override_under_the_canonical_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut cfg = mgmt_cfg(tmp.path());
        cfg.override_settings.user_settings_path = tmp.path().join("user/agents/settings.json");
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\naliases: prophet\n---\n\nBody\n",
        );

        let out = handle_management_action(&cfg, "disable", &mreq(Some("prophet"), None, Some("user"), None))
            .expect("no discovery error");
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("Disabled agent 'seer'"), "{}", out.text);

        let settings = std::fs::read_to_string(&cfg.override_settings.user_settings_path)
            .expect("settings written");
        let value: serde_json::Value = serde_json::from_str(&settings).expect("valid json");
        assert_eq!(
            value["subagents"]["agentOverrides"]["seer"]["disabled"],
            serde_json::Value::Bool(true),
            "the override must be keyed on the canonical name, not the alias: {settings}"
        );
    }

    /// A chain step that names an ALIAS is a known agent — pi swapped the `Set(names)` membership
    /// test for `resolveAgentName` in v0.43.0 (`agent-management.ts:169-174`).
    #[test]
    fn a_chain_step_naming_an_alias_does_not_warn_as_unknown() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\naliases: prophet\n---\n\nBody\n",
        );

        let config = serde_json::json!({
            "name": "foresee",
            "description": "A chain",
            "scope": "user",
            "steps": [{ "agent": "prophet", "task": "look ahead" }],
        });
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&config)))
            .expect("create ok");
        assert!(!out.is_error, "{}", out.text);
        assert!(
            !out.text.contains("unknown agents"),
            "an alias-named step must not be reported as unknown: {}",
            out.text
        );

        // Control: a step naming nothing at all still warns, so the assertion above is really
        // measuring alias resolution and not a broken warning path.
        let ghost = serde_json::json!({
            "name": "haunted",
            "description": "A chain",
            "scope": "user",
            "steps": [{ "agent": "ghost-agent", "task": "boo" }],
        });
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&ghost)))
            .expect("create ok");
        assert!(
            out.text.contains("Warning: chain steps reference unknown agents: ghost-agent."),
            "{}",
            out.text
        );
    }

    /// G101: an `extensions` list that came from `subagents.defaultExtensions` is NOT the agent's
    /// own data. `editable_base` (pi `editableAgentConfig`, `agent-management.ts:243`) must drop it
    /// so a management update never BAKES the settings default into the `.md` file — where it would
    /// outlive the setting and stop tracking it.
    #[test]
    fn a_settings_defaulted_extension_list_is_never_baked_into_the_agent_file() {
        let mut agent = sample_agent(AgentSource::User, PathBuf::from("/seer.md"));
        agent.extensions = Some(vec!["shared-ext".to_string()]);
        agent.extensions_from_default = true;

        let base = editable_base(&agent);
        assert_eq!(base.extensions, None, "a defaulted list must not survive into the edit base");
        assert!(!base.extensions_from_default);
        assert!(
            !serialize_agent(&base, None).contains("extensions:"),
            "the serialized file must carry no extensions line at all"
        );

        // An agent's OWN declared list is untouched by the same path.
        let mut own = sample_agent(AgentSource::User, PathBuf::from("/seer.md"));
        own.extensions = Some(vec!["own-ext".to_string()]);
        own.extensions_from_default = false;
        assert_eq!(editable_base(&own).extensions, Some(vec!["own-ext".to_string()]));
        assert!(serialize_agent(&editable_base(&own), None).contains("extensions: own-ext"));
    }

    /// G99: the roster is the SEVEN names pi declares at v0.43.0 (`agents.ts:38-46`), and the
    /// all-agents model report walks EXACTLY that static list.
    ///
    /// `advisor` is in the roster but ships no `advisor.md` — upstream `34a018f` demoted it to an
    /// `oracle` ALIAS — and `handleModels` looks builtins up by EXACT name
    /// (`agent-management.ts:850`, `builtinByName.get(name)`), never through `resolveAgentName`. So
    /// `advisor` renders the missing row upstream too, and this pins that the alias is not silently
    /// promoted into a seventh definition.
    #[test]
    fn the_models_report_walks_the_seven_name_roster_including_the_fileless_advisor() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "models", &mreq(None, None, None, None))
            .expect("models ok");
        assert!(!out.is_error, "{}", out.text);

        assert_eq!(
            BUILTIN_AGENT_NAMES,
            ["advisor", "delegate", "oracle", "researcher", "reviewer", "scout", "worker"]
        );
        for name in BUILTIN_AGENT_NAMES {
            assert!(out.text.contains(&format!("\n{name}\n")), "{name} row missing:\n{}", out.text);
        }
        for gone in ["planner", "context-builder"] {
            assert!(
                !out.text.contains(&format!("\n{gone}\n")),
                "the removed role {gone} must not be reported:\n{}",
                out.text
            );
        }
        assert!(
            out.text.contains("advisor\n  model:\n    (builtin definition not found)\n  source: missing"),
            "advisor ships no file of its own and must render the missing row:\n{}",
            out.text
        );
        // The six roles that DO ship a file resolve to a real definition.
        assert!(
            !out.text.contains("oracle\n  model:\n    (builtin definition not found)"),
            "{}",
            out.text
        );
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
