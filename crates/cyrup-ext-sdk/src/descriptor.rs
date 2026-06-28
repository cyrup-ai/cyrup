//! Serializable descriptors authored by a guest extension (arch-08 §3.5/§3.6). These mirror the
//! host-side registration records; `parameters` stays JSON-Schema (Pi-interop, R-ARCH-EXT-008).
//! camelCase per arch-00 §4.

use serde_json::Value;

/// Per-tool execution mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecMode {
    Parallel,
    Sequential,
}

/// What a guest sends to register a tool (R-08-012/013).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    pub label: String,
    pub description: String,
    /// JSON-Schema for the parameters.
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<ExecMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_snippet: Option<String>,
    #[serde(default)]
    pub prompt_guidelines: Vec<String>,
    #[serde(default)]
    pub has_renderer: bool,
}

impl ToolDescriptor {
    /// Minimal builder: name + JSON-Schema parameters.
    pub fn new(name: impl Into<String>, parameters: Value) -> Self {
        let name = name.into();
        Self {
            label: name.clone(),
            name,
            description: String::new(),
            parameters,
            execution_mode: None,
            prompt_snippet: None,
            prompt_guidelines: Vec::new(),
            has_renderer: false,
        }
    }

    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    pub fn prompt_snippet(mut self, s: impl Into<String>) -> Self {
        self.prompt_snippet = Some(s.into());
        self
    }
}

/// What a guest sends to register a command (R-08-016).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDescriptor {
    pub description: String,
    #[serde(default)]
    pub completions: Vec<String>,
}
