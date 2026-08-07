//! Tool registry: built-in construction, extension override, and availability filtering
//! (R-03-010/041, arch-03 §3.4).

use crate::config::ToolsOptions;
use crate::lock::FileMutationLocks;
use crate::ops::{Backend, ShellConfig};
use crate::tools::{BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool, WriteTool};
use cyrup_core::Tool;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// The closed set of built-in tool names (DI-1), in Pi's declaration order.
///
/// Pi's `createAllToolDefinitions` returns its object literal as `read, bash, edit, write, grep,
/// find, ls` (`coding-agent/src/core/tools/index.ts:156-166`), and object-literal insertion order is
/// the order `Object.values()` / the tool registry replays. That order reaches the wire: it is the
/// order of the `tools` array in every provider request and of the tool list rendered into the
/// system prompt, both of which the model conditions on.
pub const BUILTIN_NAMES: [&str; 7] = ["read", "bash", "edit", "write", "grep", "find", "ls"];

/// Model-visible tool-set controls (R-03-010).
#[derive(Clone, Debug, Default)]
pub enum Availability {
    /// Every registered tool.
    #[default]
    All,
    /// Only these names.
    Allow(HashSet<String>),
    /// All registered tools except these names.
    Exclude(HashSet<String>),
    /// Drop the seven built-ins; keep extension tools.
    NoBuiltins,
    /// Empty model-visible set.
    NoTools,
}

/// A name-keyed registry; last insert wins (override), with stable presentation order.
#[derive(Default)]
pub struct ToolRegistry {
    by_name: HashMap<String, Arc<dyn Tool>>,
    order: Vec<String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { by_name: HashMap::new(), order: Vec::new() }
    }

    /// Build the default registry with the seven built-ins over `backend` (arch-03 §3.4).
    pub fn with_builtins(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Self {
        let mut reg = Self::new();
        let locks = Arc::new(FileMutationLocks::new());
        let shell = ShellConfig::detect();

        // Insertion order IS presentation order (see `insert`/`all`/`visible` below), and it must be
        // Pi's `createAllToolDefinitions` literal order — read, bash, edit, write, grep, find, ls
        // (`coding-agent/src/core/tools/index.ts:156-166`). It also fixes the two derived sets for
        // free: filtering this order to {read,bash,edit,write} reproduces `createCodingTools`
        // (index.ts:169-176) and to {read,grep,find,ls} reproduces `createReadOnlyToolDefinitions`
        // (index.ts:147-154).
        reg.insert(Arc::new(ReadTool::new(backend.fs.clone(), cwd.clone(), opts.read)));
        reg.insert(Arc::new(BashTool::new(backend.proc.clone(), shell, cwd.clone(), opts.bash)));
        reg.insert(Arc::new(EditTool::new(
            backend.fs.clone(),
            locks.clone(),
            cwd.clone(),
            opts.edit,
        )));
        reg.insert(Arc::new(WriteTool::new(
            backend.fs.clone(),
            locks.clone(),
            cwd.clone(),
            opts.write,
        )));
        reg.insert(Arc::new(GrepTool::new(backend.fs.clone(), cwd.clone(), opts.grep)));
        reg.insert(Arc::new(FindTool::new(backend.fs.clone(), cwd.clone(), opts.find)));
        reg.insert(Arc::new(LsTool::new(backend.fs.clone(), cwd, opts.ls)));
        reg
    }

    /// Insert (or override by name, R-03-041). A new name keeps insertion order; an existing name
    /// replaces the tool while preserving its position.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        if !self.by_name.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.by_name.insert(name, tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.by_name.get(name).cloned()
    }

    /// All registered tools in presentation order.
    pub fn all(&self) -> Vec<Arc<dyn Tool>> {
        self.order.iter().filter_map(|n| self.by_name.get(n).cloned()).collect()
    }

    /// The model-visible tool set under `ctrl` (R-03-010).
    pub fn visible(&self, ctrl: &Availability) -> Vec<Arc<dyn Tool>> {
        let is_builtin = |n: &str| BUILTIN_NAMES.contains(&n);
        self.order
            .iter()
            .filter_map(|n| self.by_name.get(n).map(|t| (n.as_str(), t.clone())))
            .filter(|(n, _)| match ctrl {
                Availability::All => true,
                Availability::Allow(set) => set.contains(*n),
                Availability::Exclude(set) => !set.contains(*n),
                Availability::NoBuiltins => !is_builtin(n),
                Availability::NoTools => false,
            })
            .map(|(_, t)| t)
            .collect()
    }
}

/// Default coding tool set (read/bash/edit/write).
pub fn coding_tools(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Vec<Arc<dyn Tool>> {
    let reg = ToolRegistry::with_builtins(cwd, backend, opts);
    let allow: HashSet<String> =
        ["read", "bash", "edit", "write"].into_iter().map(String::from).collect();
    reg.visible(&Availability::Allow(allow))
}

/// Read-only tool set (read/grep/find/ls).
pub fn read_only_tools(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Vec<Arc<dyn Tool>> {
    let reg = ToolRegistry::with_builtins(cwd, backend, opts);
    let allow: HashSet<String> =
        ["read", "grep", "find", "ls"].into_iter().map(String::from).collect();
    reg.visible(&Availability::Allow(allow))
}

/// All seven built-in tools.
pub fn all_tools(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Vec<Arc<dyn Tool>> {
    ToolRegistry::with_builtins(cwd, backend, opts).all()
}
