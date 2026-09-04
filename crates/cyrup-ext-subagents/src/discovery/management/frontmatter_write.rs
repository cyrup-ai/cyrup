//! Frontmatter serialization (write-back) — a faithful port of pi's
//! `serializeAgent(config, { preserveFrontmatterFields })` (`agent-serializer.ts:37-110`). Split
//! out of `discovery/management.rs`'s own "Frontmatter serialization (write-back)" section.
//!
//! Two round-trip properties this writer upholds (T7 §3):
//!   * PRESERVE-FRONTMATTER-FIELDS on UPDATE: when `preserve_fields` is `Some`, a key that was
//!     present on disk but is NOT being changed by this update is re-emitted even if it would
//!     otherwise be omitted, so an update never silently drops the file's existing field set. On
//!     CREATE (`None`) the default field set is emitted (systemPromptMode/inheritProjectContext/
//!     inheritSkills always present).
//!   * BLOCK-VALUED EXTRA FIELDS survive rewrite: an `extra_fields` value with embedded newlines
//!     (e.g. a `permission:` nested-YAML block captured by `frontmatter.rs`) is re-emitted as
//!     `key:` + two-space-indented lines, NOT corrupted into one flat line.
//!
//! Settings-override values are NOT baked into files: the update handler feeds `serialize_agent` the
//! pre-override `editable_base` snapshot (pi `editableAgentConfig`, `agent-management.ts:217-267`),
//! and `disabled` is never emitted at all (it is a settings-only concept — a `disabled:` in an agent
//! file is just an unknown extra field, round-tripped through `extra_fields`).

use std::collections::HashSet;
use std::path::Path;

use super::super::types::{AgentDefinition, SystemPromptMode, ToolRef};
use super::agent_crud::AgentFields;
use crate::error::SubagentError;
use crate::fork_context::ContextMode;

pub(crate) fn write_agent_file(
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

pub(crate) fn serialize_agent(def: &AgentDefinition, preserve_fields: Option<&HashSet<String>>) -> String {
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

    // SUBA-092 excludeTools (`agent-serializer.ts:74-75` @v0.64.0): `joinComma(config.excludeTools)`
    // — `undefined` for an absent OR empty list — emitted when truthy or under preserve. Same
    // silent-deletion trap as `toolBudget`/`turnBudget` below: the key is in `KNOWN_FIELDS`, so
    // without this arm the first management rewrite would delete an author's exclusion list.
    let exclude_tools_value = def
        .exclude_tools
        .as_ref()
        .filter(|list| !list.is_empty())
        .map(|list| list.join(", "));
    if exclude_tools_value.is_some() || preserve(&["excludeTools"]) {
        lines.push(format!("excludeTools: {}", exclude_tools_value.as_deref().unwrap_or("")));
    }
    // SUBA-092 allowNestedSubagents (`agent-serializer.ts:76-78` @v0.64.0): emitted only when
    // `=== true` or under preserve, and under preserve an unset value is written as an EMPTY value
    // (never as `false`) while an explicit `false` is written back as `false`.
    if def.allow_nested_subagents == Some(true) || preserve(&["allowNestedSubagents"]) {
        let value = match def.allow_nested_subagents {
            None => "",
            Some(true) => "true",
            Some(false) => "false",
        };
        lines.push(format!("allowNestedSubagents: {value}"));
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

    // SUBA-008 turnBudget (`agent-serializer.ts:91` @v0.43.0): emitted as compact JSON when set,
    // or as an empty value under preserve — the same shape as `toolBudget` above, and landed with
    // it for the same reason: `turnBudget` is now in `KNOWN_FIELDS`, so a known-but-unemitted key
    // would be SILENTLY DELETED from an author's agent file on the first management rewrite.
    //
    // Upstream's guard is `config.defaultTurnBudget || preserve(...)` — TRUTHY, not
    // `!== undefined` like the two lines above it — which for an always-populated object type is
    // the same test `is_some()` performs here.
    if def.default_turn_budget.is_some() || preserve(&["turnBudget"]) {
        let value = def
            .default_turn_budget
            .as_ref()
            .and_then(|b| serde_json::to_string(b).ok())
            .unwrap_or_default();
        lines.push(format!("turnBudget: {value}"));
    }

    // SUBA-073 permission/permissions (`agent-serializer.ts` @v0.57.0): emitted as compact JSON
    // under the canonical `permissions:` key — matching `aliases`'s own precedent of always
    // writing the newer/plural spelling regardless of which the original file used — or as an
    // empty value under preserve. Same silent-deletion trap as `toolBudget`/`turnBudget` above,
    // now that `permission`/`permissions` are both in `KNOWN_FIELDS`.
    if def.permission_rules.is_some() || preserve(&["permission", "permissions"]) {
        let value = def
            .permission_rules
            .as_ref()
            .map(crate::exec::permissions::permission_rules_to_json_string)
            .unwrap_or_default();
        lines.push(format!("permissions: {value}"));
    }

    // SUBA-074 runner (`agent-serializer.ts` @v0.57.0): emitted as compact JSON, or as an empty
    // value under preserve. Same silent-deletion trap as `toolBudget`/`turnBudget`/`permissions`
    // above, now that `runner` is in `KNOWN_FIELDS`.
    if def.runner.is_some() || preserve(&["runner"]) {
        let value = def
            .runner
            .as_ref()
            .map(crate::runner::runner_to_json_string)
            .unwrap_or_default();
        lines.push(format!("runner: {value}"));
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
pub(crate) fn preserved_frontmatter_fields(
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;
    use super::super::test_support::sample_agent;
    use crate::discovery::types::AgentSource;

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

    /// SUBA-008 — the same silent-deletion trap for `turnBudget`, and the parse half with it.
    ///
    /// The trap is specific and worth naming: adding a key to `KNOWN_FIELDS` without a matching
    /// `serialize_agent` arm makes the extra-fields round-trip loop skip it, so the FIRST
    /// management rewrite of an author's agent file silently deletes their budget. Adding the key
    /// and the arm in one change is why both halves are asserted here.
    #[test]
    fn serialize_agent_round_trips_the_turn_budget_launch_default() {
        use crate::discovery::frontmatter::parse_agent_file;

        let mut def = sample_agent(AgentSource::Project, PathBuf::from("/w.md"));
        def.local_name = "pacer".to_string();
        def.name = "pacer".to_string();
        def.description = "Paces itself".to_string();
        def.system_prompt_body = "Do work".to_string();
        def.default_turn_budget = Some(crate::exec::turn_budget::ResolvedTurnBudget {
            max_turns: 6,
            grace_turns: 0,
        });

        let serialized = serialize_agent(&def, None);
        assert!(
            serialized.contains(r#"turnBudget: {"maxTurns":6,"graceTurns":0}"#),
            "turnBudget must be emitted as compact JSON under pi's own key names:\n{serialized}"
        );

        let reparsed = parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))
            .expect("round-trips back through the parser");
        assert_eq!(
            reparsed.default_turn_budget, def.default_turn_budget,
            "turnBudget lost on round trip — the silent-deletion trap"
        );
        assert!(
            !reparsed.extra_fields.contains_key("turnBudget"),
            "a KNOWN_FIELDS key must never be demoted to extra_fields"
        );

        // The parser applies upstream's `graceTurns` DEFAULT, so an author who writes only
        // `maxTurns` gets grace 1 — not grace 0, which is a materially different policy.
        let with_default = parse_agent_file(
            "---\nname: pacer\ndescription: d\nturnBudget: {\"maxTurns\": 3}\n---\nbody",
            AgentSource::Project,
            Path::new("/w.md"),
        )
        .expect("parses");
        assert_eq!(
            with_default.default_turn_budget,
            Some(crate::exec::turn_budget::ResolvedTurnBudget {
                max_turns: 3,
                grace_turns: 1,
            })
        );

        // A malformed budget skips the FILE (this crate's per-file `[CYRUP-DELTA]`), it does not
        // silently disarm the budget and load the agent anyway.
        assert!(
            parse_agent_file(
                "---\nname: pacer\ndescription: d\nturnBudget: {\"maxTurns\": 0}\n---\nbody",
                AgentSource::Project,
                Path::new("/w.md"),
            )
            .is_none(),
            "an invalid turnBudget must skip the agent file, never load it unbudgeted"
        );
    }

    /// SUBA-073 — the same silent-deletion trap for `permission`/`permissions`: now that both
    /// spellings are `KNOWN_FIELDS`, a management rewrite that never emits them would otherwise
    /// silently delete an author's policy on the first `update_agent` call.
    #[test]
    fn serialize_agent_round_trips_the_permission_policy() {
        use crate::discovery::frontmatter::parse_agent_file;
        use crate::watchdog::permission_arbiter::{PermissionRuleDecision, PermissionRules};

        let mut def = sample_agent(AgentSource::Project, PathBuf::from("/w.md"));
        def.local_name = "warden".to_string();
        def.name = "warden".to_string();
        def.description = "Guards things".to_string();
        def.system_prompt_body = "Do work".to_string();
        let mut rules = PermissionRules::new();
        rules.insert("write".to_string(), PermissionRuleDecision::Deny);
        def.permission_rules = Some(rules);

        let serialized = serialize_agent(&def, None);
        assert!(
            serialized.contains(r#"permissions: {"write":"deny"}"#),
            "the policy must be emitted as compact JSON under the canonical plural key:\n{serialized}"
        );

        let reparsed = parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))
            .expect("round-trips back through the parser");
        assert_eq!(
            reparsed.permission_rules, def.permission_rules,
            "the permission policy must survive a serialize -> re-parse round trip"
        );
        assert!(
            !reparsed.extra_fields.contains_key("permission")
                && !reparsed.extra_fields.contains_key("permissions"),
            "a KNOWN_FIELDS key must never be demoted to extra_fields"
        );

        // An update that never mentions the policy must not DROP an existing one under preserve.
        let mut preserve = HashSet::new();
        preserve.insert("permissions".to_string());
        let mut unset_def = def.clone();
        unset_def.permission_rules = None;
        let preserved_serialized = serialize_agent(&unset_def, Some(&preserve));
        assert!(
            preserved_serialized.contains("permissions:"),
            "an unrelated update must preserve the existing permissions: line:\n{preserved_serialized}"
        );
    }

    /// SUBA-074 — the same silent-deletion trap for `runner:`: now that it is a `KNOWN_FIELD`, a
    /// serializer that never emits it would drop an author's runner block on the first management
    /// rewrite.
    #[test]
    fn serialize_agent_round_trips_the_runner_block() {
        use crate::discovery::frontmatter::parse_agent_file;
        use crate::runner::{AgentRunnerConfig, ExternalCliRunner};

        let mut def = sample_agent(AgentSource::Project, PathBuf::from("/w.md"));
        def.local_name = "worker".to_string();
        def.name = "worker".to_string();
        def.description = "Worker".to_string();
        def.system_prompt_body = "Do work".to_string();
        def.runner = Some(AgentRunnerConfig::ExternalCli(ExternalCliRunner {
            adapter: Some("claude-code".to_string()),
            command: "claude".to_string(),
            args: Vec::new(),
            prompt_delivery_stdin: false,
            capabilities: None,
        }));

        let serialized = serialize_agent(&def, None);
        assert!(
            serialized
                .contains(r#"runner: {"type":"external-cli","adapter":"claude-code","command":"claude"}"#),
            "the runner must be emitted as compact JSON in upstream's key order:\n{serialized}"
        );

        let reparsed = parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))
            .expect("round-trips back through the parser");
        assert_eq!(
            reparsed.runner, def.runner,
            "the runner block must survive a serialize -> re-parse round trip"
        );
        assert!(
            !reparsed.extra_fields.contains_key("runner"),
            "a KNOWN_FIELDS key must never be demoted to extra_fields"
        );

        // An unrelated update must not DROP an existing runner under preserve.
        let mut preserve = HashSet::new();
        preserve.insert("runner".to_string());
        let mut unset = def.clone();
        unset.runner = None;
        assert!(
            serialize_agent(&unset, Some(&preserve)).contains("runner:"),
            "an unrelated update must preserve the existing runner: line"
        );
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
        // field (embedded newlines, e.g. a `vendorPolicy:` nested-YAML block) is re-emitted as an
        // indented block rather than corrupted into a flat line.
        //
        // SUBA-073: this test used `permission:` as its stand-in "unknown key with a block value"
        // example. `permission`/`permissions` are now KNOWN_FIELDS (validated, typed), so they are
        // no longer a valid example of an unrecognized key — swapped for a fictitious
        // `vendorPolicy:` key that stays genuinely unknown, preserving this test's real subject
        // (the block-value round-trip mechanic itself, not permission's own semantics).
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
            "vendorPolicy".to_string(),
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
                "vendorPolicy:\n  \"*\": ask\n  read: allow\n  bash:\n    \"*\": ask\n    \"git *\": allow"
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
            reparsed.extra_fields.get("vendorPolicy").map(String::as_str),
            Some("\"*\": ask\nread: allow\nbash:\n  \"*\": ask\n  \"git *\": allow"),
            "the block value must round-trip byte-for-byte"
        );
        assert_eq!(reparsed.extra_fields.get("disabled").map(String::as_str), Some("true"));
        assert_eq!(reparsed.disabled, None, "disabled: in a file is never an honored flag");
        assert_eq!(reparsed.thinking, Some("off".to_string()));
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


    /// SUBA-092 — the same silent-deletion trap as `toolBudget`/`turnBudget` above, for the two keys
    /// `b26da18e` added to `KNOWN_FIELDS` (`agent-serializer.ts:12-13,74-78` @v0.64.0). Both halves
    /// are asserted: the emit arms, and the round-trip back through the parser.
    #[test]
    fn serialize_agent_round_trips_exclude_tools_and_allow_nested_subagents() {
        use crate::discovery::frontmatter::parse_agent_file;

        let mut def = sample_agent(AgentSource::Project, PathBuf::from("/w.md"));
        def.local_name = "worker".to_string();
        def.name = "worker".to_string();
        def.description = "Works".to_string();
        def.system_prompt_body = "Do work".to_string();
        def.exclude_tools = Some(vec!["bash".to_string(), "write".to_string()]);
        def.allow_nested_subagents = Some(true);

        let serialized = serialize_agent(&def, None);
        assert!(
            serialized.contains("\nexcludeTools: bash, write\n"),
            "excludeTools must be emitted as a comma list:\n{serialized}"
        );
        assert!(
            serialized.contains("\nallowNestedSubagents: true\n"),
            "allowNestedSubagents: true must be emitted:\n{serialized}"
        );

        let reparsed = parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))
            .expect("round-trips back through the parser");
        assert_eq!(reparsed.exclude_tools, def.exclude_tools, "excludeTools lost on round trip");
        assert_eq!(reparsed.allow_nested_subagents, Some(true), "allowNestedSubagents lost on round trip");
        assert!(!reparsed.extra_fields.contains_key("excludeTools"));
        assert!(!reparsed.extra_fields.contains_key("allowNestedSubagents"));
    }

    /// `agent-serializer.ts:74-78` @v0.64.0, the non-truthy arms: an empty/absent `excludeTools` and
    /// an unset or `false` `allowNestedSubagents` are NOT emitted on a fresh serialize; under
    /// preserve, an absent value is written as an EMPTY value and an explicit `false` as `false`.
    #[test]
    fn serialize_agent_emits_the_two_suba092_keys_only_when_truthy_or_preserved() {
        let mut def = sample_agent(AgentSource::Project, PathBuf::from("/w.md"));
        def.exclude_tools = Some(Vec::new());
        def.allow_nested_subagents = Some(false);
        let fresh = serialize_agent(&def, None);
        assert!(
            !fresh.contains("excludeTools") && !fresh.contains("allowNestedSubagents"),
            "neither key is truthy, so neither is emitted on a create:\n{fresh}"
        );

        let preserve: HashSet<String> = ["excludeTools", "allowNestedSubagents"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let preserved = serialize_agent(&def, Some(&preserve));
        assert!(preserved.contains("\nexcludeTools: \n"), "empty value under preserve:\n{preserved}");
        assert!(
            preserved.contains("\nallowNestedSubagents: false\n"),
            "an explicit false is written back as false under preserve:\n{preserved}"
        );

        def.allow_nested_subagents = None;
        let preserved = serialize_agent(&def, Some(&preserve));
        assert!(
            preserved.contains("\nallowNestedSubagents: \n"),
            "an UNSET value under preserve is an empty value, never `false`:\n{preserved}"
        );
    }
}
