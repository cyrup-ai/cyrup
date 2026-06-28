//! The extension registry (arch-08 §3.5, registry/*): the tables of tools / commands / shortcuts /
//! flags / providers / renderers an extension contributes. Tool registration overrides a built-in
//! of the same name (R-08-012); the merged active set is handed to the agent each run (R-08-014).

use crate::error::ExtError;
use cyrup_core::{ExecMode, ExtensionId, Tool};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// What a guest sends to register a tool (arch-08 §3.5). `parameters` stays JSON-Schema (Pi-interop,
/// R-ARCH-EXT-008). camelCase per arch-00 §4.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    pub label: String,
    pub description: String,
    /// JSON-Schema for the tool parameters.
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<ExecModeWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_snippet: Option<String>,
    #[serde(default)]
    pub prompt_guidelines: Vec<String>,
    #[serde(default)]
    pub has_renderer: bool,
}

impl ToolDescriptor {
    /// Validate the descriptor at registration (R-ARCH-EXT-008): `name` non-empty and `parameters`
    /// is a JSON object (a minimal JSON-Schema validity check; full schema validation is a follow-on).
    pub fn validate(&self) -> Result<(), ExtError> {
        if self.name.trim().is_empty() {
            return Err(ExtError::Schema("tool name is empty".into()));
        }
        if !self.parameters.is_object() {
            return Err(ExtError::Schema(format!(
                "tool `{}` parameters must be a JSON-Schema object",
                self.name
            )));
        }
        Ok(())
    }
}

/// Wire form of `ExecMode` (serde camelCase).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecModeWire {
    Parallel,
    Sequential,
}

impl From<ExecModeWire> for ExecMode {
    fn from(w: ExecModeWire) -> Self {
        match w {
            ExecModeWire::Parallel => ExecMode::Parallel,
            ExecModeWire::Sequential => ExecMode::Sequential,
        }
    }
}

/// A registered command descriptor (R-08-016). The handler runs with a command-tier ctx.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDescriptor {
    pub description: String,
    #[serde(default)]
    pub completions: Vec<String>,
}

/// The registry of everything extensions contribute. `Send + Sync` (interior `RwLock`), shared via
/// `Arc` across the host (arch-08 §3.1).
#[derive(Default)]
pub struct ExtensionRegistry {
    inner: RwLock<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    /// Extension tools keyed by name; last insert wins (override, R-08-012). Insertion order kept.
    tool_order: Vec<String>,
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Which extension owns each tool name (for diagnostics / unload).
    tool_owner: HashMap<String, ExtensionId>,
    commands: HashMap<String, (ExtensionId, CommandDescriptor)>,
    shortcuts: HashMap<String, ExtensionId>,
    flags: HashMap<String, Value>,
    providers: HashMap<String, (ExtensionId, Value)>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or override) an extension tool. A `PoisonError` degrades to a surfaced error,
    /// never a panic (R-00-009).
    pub fn register_tool(&self, owner: ExtensionId, tool: Arc<dyn Tool>) -> Result<(), ExtError> {
        let name = tool.name().to_string();
        let mut g = self.lock_write()?;
        if !g.tools.contains_key(&name) {
            g.tool_order.push(name.clone());
        }
        g.tool_owner.insert(name.clone(), owner);
        g.tools.insert(name, tool);
        Ok(())
    }

    pub fn register_command(
        &self,
        owner: ExtensionId,
        name: impl Into<String>,
        desc: CommandDescriptor,
    ) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.commands.insert(name.into(), (owner, desc));
        Ok(())
    }

    /// Register a keyboard shortcut owned by an extension (R-08-017).
    pub fn register_shortcut(
        &self,
        owner: ExtensionId,
        key: impl Into<String>,
    ) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.shortcuts.insert(key.into(), owner);
        Ok(())
    }

    /// Keys with a registered shortcut.
    pub fn shortcut_keys(&self) -> Result<Vec<String>, ExtError> {
        Ok(self.lock_read()?.shortcuts.keys().cloned().collect())
    }

    pub fn register_provider(
        &self,
        owner: ExtensionId,
        id: impl Into<String>,
        config: Value,
    ) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.providers.insert(id.into(), (owner, config));
        Ok(())
    }

    pub fn unregister_provider(&self, id: &str) -> Result<bool, ExtError> {
        let mut g = self.lock_write()?;
        Ok(g.providers.remove(id).is_some())
    }

    pub fn set_flag(&self, name: impl Into<String>, spec: Value) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.flags.insert(name.into(), spec);
        Ok(())
    }

    pub fn get_flag(&self, name: &str) -> Result<Option<Value>, ExtError> {
        Ok(self.lock_read()?.flags.get(name).cloned())
    }

    /// All extension tools in registration order (overrides resolved).
    pub fn extension_tools(&self) -> Result<Vec<Arc<dyn Tool>>, ExtError> {
        let g = self.lock_read()?;
        Ok(g.tool_order.iter().filter_map(|n| g.tools.get(n).cloned()).collect())
    }

    /// Look up an extension tool by name.
    pub fn tool(&self, name: &str) -> Result<Option<Arc<dyn Tool>>, ExtError> {
        Ok(self.lock_read()?.tools.get(name).cloned())
    }

    pub fn has_command(&self, name: &str) -> Result<bool, ExtError> {
        Ok(self.lock_read()?.commands.contains_key(name))
    }

    pub fn provider_ids(&self) -> Result<Vec<String>, ExtError> {
        Ok(self.lock_read()?.providers.keys().cloned().collect())
    }

    /// Merge a base tool set (built-ins) with extension tools; extension tools override by name
    /// (R-08-012/014). Stable order: base order first, then new extension-only tools.
    pub fn active_tools(&self, base: &[Arc<dyn Tool>]) -> Result<Vec<Arc<dyn Tool>>, ExtError> {
        let g = self.lock_read()?;
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for t in base {
            let name = t.name().to_string();
            seen.insert(name.clone());
            if let Some(over) = g.tools.get(&name) {
                out.push(over.clone());
            } else {
                out.push(t.clone());
            }
        }
        for n in &g.tool_order {
            if !seen.contains(n) {
                if let Some(t) = g.tools.get(n) {
                    out.push(t.clone());
                }
            }
        }
        Ok(out)
    }

    fn lock_read(&self) -> Result<std::sync::RwLockReadGuard<'_, RegistryInner>, ExtError> {
        self.inner.read().map_err(|_| ExtError::Io("registry lock poisoned".into()))
    }

    fn lock_write(&self) -> Result<std::sync::RwLockWriteGuard<'_, RegistryInner>, ExtError> {
        self.inner.write().map_err(|_| ExtError::Io("registry lock poisoned".into()))
    }
}
