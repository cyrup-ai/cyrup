//! Chain create/update/delete/rename (R-SA-014 applies identically to chains). Split out of
//! `discovery/management.rs`'s own "Chain create/update/delete/rename" section. Fully
//! self-contained: every helper here is called only from within this file's own functions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::super::types::{AgentSource, ChainDefinition, ChainStepConfig};
use super::visibility::require_writable_source;
use crate::error::SubagentError;

/// The caller-supplied delta for a chain create/update — deliberately minimal. This module's own
/// job (R-SA-013/014/019 CRUD + visibility) does not include full chain-step authoring (building
/// up a real [`ChainStepConfig`] sequence is a chain-editor concern, not a bare-CRUD concern) —
/// `step_count` only controls how many placeholder [`ChainStepConfig`] entries this module
/// materializes when it needs to preserve or resize a chain's step list without inventing per-step
/// content of its own. A future chain-editor-facing API (outside this file's R-SA-013/014/019
/// scope) would supply real [`ChainStepConfig`] values directly rather than going through
/// `step_count`.
#[derive(Clone, Debug, Default)]
#[allow(
    dead_code,
    reason = "exercised only by this file's own direct-primitive unit tests below \
    (create_chain_with_steps/update_chain_full below are the real production entry points, called \
    from handlers.rs); pre-existing asymmetry preserved verbatim from the original flat management.rs, \
    where these were silently externally-reachable through the top-level `pub mod management` and so \
    never tripped dead_code — nesting them in this private submodule makes that reachability honest"
)]
pub struct ChainFields {
    pub name: Option<String>,
    pub description: Option<String>,
    pub step_count: Option<usize>,
}

/// Build one minimal, empty placeholder [`ChainStepConfig`] — used only to preserve step *count*
/// across a management-layer chain update that does not itself author step content (see
/// [`ChainFields`]'s own doc). Every field left at its default (`None`/empty) so this placeholder
/// carries no spurious behavior if ever (mis)dispatched directly.
#[allow(dead_code, reason = "see ChainFields' own allow(dead_code) note above")]
fn placeholder_chain_step() -> ChainStepConfig {
    ChainStepConfig::default()
}

/// Create a new `.chain.json` file under `scope_dir` (R-SA-014: `source` must be `User`/
/// `Project`). Chain names have no package-identifier concept (R-SA-006 does not apply to
/// chains), so unlike `agent_crud::create_agent` this function has no silent-skip return
/// path — it either succeeds or returns a hard `Err`.
#[allow(dead_code, reason = "see ChainFields' own allow(dead_code) note above")]
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
#[allow(dead_code, reason = "see ChainFields' own allow(dead_code) note above")]
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
#[allow(dead_code, reason = "see ChainFields' own allow(dead_code) note above")]
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
    let runtime_name =
        super::super::types::AgentDefinition::qualified_name(local_name, package_name.as_deref());
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
    let runtime_name = super::super::types::AgentDefinition::qualified_name(
        new_local_name,
        package_name.as_deref(),
    );
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
/// `crate::discovery::chains::parse_chain_json` reads (`serializeJsonChain`,
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::super::test_support::sample_chain;
    use super::*;

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
        let builtin = sample_chain(
            AgentSource::Builtin,
            PathBuf::from("/builtin/release.chain.json"),
        );
        assert!(matches!(
            update_chain(&builtin, &ChainFields::default()),
            Err(SubagentError::ReadOnlySource(_))
        ));

        let package = sample_chain(
            AgentSource::Package,
            PathBuf::from("/pkg/release.chain.json"),
        );
        assert!(matches!(
            update_chain(&package, &ChainFields::default()),
            Err(SubagentError::ReadOnlySource(_))
        ));
    }

    #[test]
    fn delete_chain_rejects_builtin_and_package_sources() {
        let builtin = sample_chain(
            AgentSource::Builtin,
            PathBuf::from("/builtin/release.chain.json"),
        );
        assert!(matches!(
            delete_chain(&builtin),
            Err(SubagentError::ReadOnlySource(_))
        ));

        let package = sample_chain(
            AgentSource::Package,
            PathBuf::from("/pkg/release.chain.json"),
        );
        assert!(matches!(
            delete_chain(&package),
            Err(SubagentError::ReadOnlySource(_))
        ));
    }

    #[test]
    fn rename_chain_rejects_builtin_and_package_sources() {
        let builtin = sample_chain(
            AgentSource::Builtin,
            PathBuf::from("/builtin/release.chain.json"),
        );
        assert!(matches!(
            rename_chain(&builtin, "new-name"),
            Err(SubagentError::ReadOnlySource(_))
        ));

        let package = sample_chain(
            AgentSource::Package,
            PathBuf::from("/pkg/release.chain.json"),
        );
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
}
