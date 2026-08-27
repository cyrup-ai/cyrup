//! Shared test fixtures for `discovery/management/`'s leaf test modules — split out of the flat
//! `discovery/management.rs`'s single `#[cfg(test)] mod tests`. Only the fixtures genuinely used
//! across more than one leaf module live here (`sample_agent`/`sample_chain`, needed by
//! `visibility`, `agent_crud`, `chain_crud`, `frontmatter_write`, and `handlers`); fixtures used
//! only by `mod.rs`'s own dispatch-level integration tests (`mgmt_cfg`, `mreq`, `write_agent_md`,
//! `seed_two_agents_sharing_a_skill`) stay local to that test module instead.

#![cfg(test)]

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::PathBuf;

use super::super::types::{AgentDefinition, AgentSource, ChainDefinition, SystemPromptMode};

pub(crate) fn sample_agent(source: AgentSource, file_path: PathBuf) -> AgentDefinition {
    AgentDefinition {
        default_turn_budget: None,
        permission_rules: None,
        runner: None,
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

pub(crate) fn sample_chain(source: AgentSource, file_path: PathBuf) -> ChainDefinition {
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
