//! Tool registry: built-in construction, extension override, and availability filtering
//! (R-03-010/041, arch-03 §3.4).

use crate::config::{GrepOpts, ToolsOptions};
use crate::lock::FileMutationLocks;
use crate::ops::Backend;
use crate::tools::{EditTool, FindTool, GrepTool, LsTool, ReadTool, ShellTool, WriteTool};
use cyrup_core::Tool;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// The closed set of built-in tool names (DI-1), in Pi's declaration order.
///
/// Pi's `createAllToolDefinitions` returns its object literal as `read, bash, powershell, edit,
/// write, grep, find, ls` (`coding-agent/src/core/tools/index.ts:182-193`, matching `allToolNames`
/// at `:96-105`), and object-literal insertion order is the order `Object.values()` / the tool
/// registry replays. That order reaches the wire: it is the order of the `tools` array in every
/// provider request and of the tool list rendered into the system prompt, both of which the model
/// conditions on. `powershell` therefore goes THIRD, immediately after `bash` — not appended.
pub const BUILTIN_NAMES: [&str; 8] = [
    "read",
    "bash",
    "powershell",
    "edit",
    "write",
    "grep",
    "find",
    "ls",
];

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
    /// Drop the eight built-ins; keep extension tools.
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
        Self {
            by_name: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// Build the default registry with the eight built-ins over `backend` (arch-03 §3.4).
    ///
    /// Resolves NO shell. Pi's `createAllToolDefinitions` (index.ts:182) does not either — the only
    /// `getShellConfig` call on the bash path is inside `exec` (bash.ts:91) — so a host with no
    /// bash still gets a working registry, and its `No bash shell found` recipe arrives as the
    /// `bash` TOOL RESULT rather than aborting session construction. Making this fallible would
    /// also break `read_only_tools`, which contains no bash tool at all.
    pub fn with_builtins(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Self {
        let mut reg = Self::new();
        let locks = Arc::new(FileMutationLocks::new());

        // Insertion order IS presentation order (see `insert`/`all`/`visible` below), and it must be
        // Pi's `createAllToolDefinitions` literal order — read, bash, powershell, edit, write, grep,
        // find, ls (`coding-agent/src/core/tools/index.ts:182-193`). It also fixes the two derived
        // sets for free: filtering this order to {read,bash,edit,write} reproduces
        // `createCodingTools` (index.ts:195-202) and to {read,grep,find,ls} reproduces
        // `createReadOnlyToolDefinitions` (index.ts:173-180). Neither derived set contains
        // `powershell`, exactly as upstream.
        reg.insert(Arc::new(ReadTool::new(
            backend.fs.clone(),
            cwd.clone(),
            opts.read,
        )));
        reg.insert(Arc::new(ShellTool::bash(
            backend.proc.clone(),
            cwd.clone(),
            opts.bash,
        )));
        // Registered on EVERY platform. Pi builds the definition unconditionally
        // (`createAllToolDefinitions`, index.ts:186) and only `getPowerShellConfig` is Windows-gated
        // (shell.ts:126-128), so the tool is always NAMEABLE and reports its own refusal as a tool
        // result. Gating registration on `cfg!(windows)` would make `--tools powershell` silently
        // select nothing off-Windows instead of saying why.
        reg.insert(Arc::new(ShellTool::powershell(
            backend.proc.clone(),
            cwd.clone(),
            opts.powershell,
        )));
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
        reg.insert(Arc::new(GrepTool::new(
            backend.fs.clone(),
            cwd.clone(),
            // The one place `$RIPGREP_CONFIG_PATH` is resolved. Pi gets the user's ripgrep config
            // for free — its grep spawns the real binary, which reads the variable itself
            // (`grep.ts:226`, no `env` key, no `--no-config`) — whereas cyrup searches in-process
            // and has to hand the path in. Reading it HERE rather than inside the tool keeps the
            // tool a pure function of its options, so its tests need no env mutation (`set_var` is
            // `unsafe` under edition 2024, and would race every other test in the binary).
            //
            // An explicit path already on `opts.grep` wins, so an embedder that configures one is
            // not overridden by the ambient environment.
            GrepOpts {
                rg_config_path: opts
                    .grep
                    .rg_config_path
                    .clone()
                    .or_else(crate::tools::rg_config_path_from_env),
                ..opts.grep
            },
        )));
        reg.insert(Arc::new(FindTool::new(
            backend.fs.clone(),
            cwd.clone(),
            opts.find,
        )));
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
        self.order
            .iter()
            .filter_map(|n| self.by_name.get(n).cloned())
            .collect()
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
    let allow: HashSet<String> = ["read", "bash", "edit", "write"]
        .into_iter()
        .map(String::from)
        .collect();
    reg.visible(&Availability::Allow(allow))
}

/// Read-only tool set (read/grep/find/ls).
pub fn read_only_tools(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Vec<Arc<dyn Tool>> {
    let reg = ToolRegistry::with_builtins(cwd, backend, opts);
    let allow: HashSet<String> = ["read", "grep", "find", "ls"]
        .into_iter()
        .map(String::from)
        .collect();
    reg.visible(&Availability::Allow(allow))
}

/// All eight built-in tools.
pub fn all_tools(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Vec<Arc<dyn Tool>> {
    ToolRegistry::with_builtins(cwd, backend, opts).all()
}
