//! Agent create/update/delete/rename. Split out of `discovery/management.rs`'s own "Agent
//! create/update/delete/rename" section, plus the "Agent field-set" struct pair that precedes it
//! in the original banner ordering (they are this section's own input/output contract).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use cyrup_core::ModelId;

use super::super::types::{AgentDefinition, AgentSource, OutputSpec, SystemPromptMode, ToolRef};
use super::frontmatter_write::{preserved_frontmatter_fields, write_agent_file};
use super::helpers::normalize_package_identifier;
use super::visibility::require_writable_source;
use crate::error::SubagentError;
use crate::fork_context::ContextMode;

/// The subset of [`AgentDefinition`] fields a management create/update call may supply. `None`
/// on an `Option<T>`-typed field of this struct means "caller did not touch this field" (for
/// `update_agent`, the existing on-disk value is preserved; for `create_agent`, the
/// [`AgentDefinition`]-level default applies). This is deliberately **not** the three-state
/// [`crate::discovery::types::OverrideField`] shape — that type exists for *settings-layer* override deltas
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

pub(crate) fn agent_file_path(scope_dir: &Path, local_name: &str) -> PathBuf {
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
        // SUBA-092: no management input field exists for either yet (upstream's `agentUpdate`
        // handler accepts `config.excludeTools`, `agent-management.ts:487-497` @v0.64.0 — not
        // ported here), so a CREATED agent declares neither.
        exclude_tools: None,
        allow_nested_subagents: None,
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
        // SUBA-008: same rule as `toolBudget` — no management field exists for it, so a CREATED
        // agent declares none.
        default_turn_budget: None,
        // SUBA-073: same rule — no management field exists for it, so a CREATED agent declares
        // none (an author sets it by hand-editing the agent file's `permissions:` frontmatter).
        permission_rules: None,
        // SUBA-074: same rule — no management field exists for it either, so a CREATED agent
        // declares no runner (an author sets it by hand-editing the file's `runner:` frontmatter).
        runner: None,
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
        // SUBA-092: preserved verbatim across an update so a management rewrite never strips an
        // author's exclusion list or nested-delegation grant (`agent-management.ts:321,323`).
        exclude_tools: existing.exclude_tools.clone(),
        allow_nested_subagents: existing.allow_nested_subagents,
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
        // SUBA-008: an UPDATE never edits it but must not DROP it either — see the note above.
        default_turn_budget: existing.default_turn_budget,
        // SUBA-073: an UPDATE never edits it but must not DROP it either — see the note above.
        permission_rules: existing.permission_rules.clone(),
        // SUBA-074: an UPDATE never edits it but must not DROP it either — see the note above.
        runner: existing.runner.clone(),
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
/// indicates an internal serialization bug in [`crate::discovery::management::frontmatter_write::write_agent_file`],
/// surfaced as a `Spawn`-flavored I/O error rather than silently returning a definition that does
/// not match what was written.
fn reparse_agent_file(file_path: &Path, source: AgentSource) -> Result<AgentDefinition, SubagentError> {
    let content = std::fs::read_to_string(file_path).map_err(SubagentError::Spawn)?;
    crate::discovery::frontmatter::parse_agent_file(&content, source, file_path).ok_or_else(|| {
        SubagentError::Spawn(std::io::Error::other(format!(
            "internal error: just-written agent file at {} failed to re-parse",
            file_path.display()
        )))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use super::super::test_support::sample_agent;

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
}
