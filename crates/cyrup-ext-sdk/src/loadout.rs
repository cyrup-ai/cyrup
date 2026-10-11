//! The guest half of pi's `ToolDefinition.prepareLoadout` (`extensions/types.ts:539-563`
//! @v1.0.4): the [`ToolLoadout`] the host hands a tool's [`crate::ToolExec::prepare_loadout`] and
//! the [`ToolLoadoutChanges`] it answers with.
//!
//! Pi's `ToolLoadout` is an object with three tool arrays and three lookup closures. A closure
//! cannot cross the component boundary, so the host sends the three arrays as rows that already
//! carry what the lookups return (`exposure`, `namespace`, `promptGuidelines`), and
//! [`ToolLoadout::exposure`], [`ToolLoadout::namespace`] and [`ToolLoadout::prompt_guidelines`] are
//! the lookups over them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ctx::ToolNamespaceInfo;
use crate::descriptor::ToolExposure;

/// One tool of a [`ToolLoadout`] — the parts of pi's `AgentTool` a loadout hook reads, plus the
/// tool's normalized `promptGuidelines`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadoutTool {
    /// The tool's name.
    pub name: String,
    /// The tool's display label.
    #[serde(default)]
    pub label: Option<String>,
    /// The tool's original description (what `descriptions` in [`ToolLoadoutChanges`] replaces).
    #[serde(default)]
    pub description: String,
    /// JSON Schema of its arguments.
    #[serde(default)]
    pub parameters: Value,
    /// JSON Schema of its `structuredContent`, for a tool that declares one.
    #[serde(default)]
    pub output_schema: Option<Value>,
    /// Pi's exposure literal: `direct`, `model-only`, `codemode`, `deferred` or `hidden`.
    #[serde(default)]
    pub exposure: String,
    /// The group the tool belongs to.
    #[serde(default)]
    pub namespace: Option<ToolNamespaceInfo>,
    /// The tool's `promptGuidelines`, trimmed, without empty or repeated ones.
    #[serde(default)]
    pub prompt_guidelines: Vec<String>,
}

/// The tools of a session as a [`crate::ToolExec::prepare_loadout`] hook sees them (pi
/// `ToolLoadout`, `extensions/types.ts:540-551` @v1.0.4).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ToolLoadout {
    /// Tools declared to the model (the active tools), in order, with their original descriptions.
    #[serde(default)]
    pub declared: Vec<LoadoutTool>,
    /// Tools callable through `ctx.execute_tool()`.
    #[serde(default)]
    pub callable: Vec<LoadoutTool>,
    /// Every registered tool.
    #[serde(default)]
    pub registered: Vec<LoadoutTool>,
}

impl ToolLoadout {
    fn registered_tool(&self, name: &str) -> Option<&LoadoutTool> {
        self.registered.iter().rev().find(|t| t.name == name)
    }

    /// The exposure of the registered tool `name` (pi `getExposure`); an unknown name is `direct`,
    /// as upstream's `?? "direct"`.
    pub fn exposure(&self, name: &str) -> ToolExposure {
        self.registered_tool(name)
            .and_then(|t| serde_json::from_value(Value::String(t.exposure.clone())).ok())
            .unwrap_or_default()
    }

    /// The namespace of the registered tool `name`, if it has one (pi `getNamespace`).
    pub fn namespace(&self, name: &str) -> Option<&ToolNamespaceInfo> {
        self.registered_tool(name)?.namespace.as_ref()
    }

    /// The prompt guidelines of the registered tool `name` (pi `getPromptGuidelines`); an unknown
    /// name has none.
    pub fn prompt_guidelines(&self, name: &str) -> &[String] {
        self.registered_tool(name)
            .map_or(&[], |t| t.prompt_guidelines.as_slice())
    }
}

/// Changes a [`crate::ToolExec::prepare_loadout`] hook makes to what the model sees (pi
/// `ToolLoadoutChanges`, `extensions/types.ts:553-563` @v1.0.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolLoadoutChanges {
    /// Model-facing descriptions of declared tools, by tool name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub descriptions: BTreeMap<String, String>,
    /// Declared tools whose declarations requests leave out. They stay active and callable, and the
    /// transcript still declares them, so the active set survives `/tree` and resume.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_declarations: Vec<String>,
}

impl ToolLoadoutChanges {
    /// No changes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the model-facing description of the declared tool `name` (builder-style).
    #[must_use]
    pub fn describe(mut self, name: impl Into<String>, description: impl Into<String>) -> Self {
        self.descriptions.insert(name.into(), description.into());
        self
    }

    /// Leave the declared tool `name` out of requests (builder-style).
    #[must_use]
    pub fn hide(mut self, name: impl Into<String>) -> Self {
        self.hidden_declarations.push(name.into());
        self
    }
}
