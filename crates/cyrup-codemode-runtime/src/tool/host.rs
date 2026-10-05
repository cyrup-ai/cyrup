//! What the `codemode` tool needs from the session it runs in.
//!
//! Upstream's tool is handed an `ExtensionToolContext` (`ctx`) per call: `ctx.tools` (the callable
//! tools), `ctx.executeTool` (a nested call through the agent loop's pipeline), `ctx.sessionManager`
//! (the branch the store replays over), `ctx.modelRegistry`, and `options.appendEntry` /
//! `getMode` / `getInlineBudget` from the extension (`execute.ts:316-347`, `index.ts:31-45`
//! @v1.0.1). A cyrup tool's `execute` receives no context, so those are one trait, implemented by
//! the session and bound into a [`CodemodeHostSlot`] that the tool and its extension share.
//!
//! Upstream's `getToolNamespace` and `ctx.cwd` have no counterpart here on purpose: a callable
//! tool is an `Arc<dyn Tool>` that already knows its [`Tool::namespace`], and nothing the tool
//! does reads the working directory (the output spill is the OS temp directory).
//!
//! # Production call path
//!
//! The session implements [`CodemodeHost`] (`cyrup-session-svc`, `session/codemode.rs`) and binds
//! it with [`CodemodeHostSlot::bind`] when the session is built; [`super::CodemodeTool`] reads
//! [`CodemodeHostSlot::current`] on every `execute` and every loadout resolution.

use std::sync::{Arc, Mutex, PoisonError};

use cyrup_config::CodemodeMode;
use cyrup_core::{CancelToken, Tool, ToolCallId, ToolResult};
use serde_json::Value;

use super::models::CodemodeModels;
use super::store::{BranchCustomEntry, CodemodeStoreEntryData};

/// How a nested call came back: the agent loop's `AgentToolCallOutcome` (`{ toolCall, result,
/// isError }`) reduced to what a script needs. A call that cannot run (unknown tool, a tool that
/// is not callable, invalid arguments, a blocked call) is an outcome with `is_error` set, never an
/// `Err`.
#[derive(Clone, Debug)]
pub struct NestedOutcome {
    /// The id the call ran under, `<codemode call id>/<n>`.
    pub call_id: ToolCallId,
    pub result: ToolResult,
    pub is_error: bool,
}

/// The session refused to record a `store()` write. Carries the session's own message.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct StoreAppendFailed(pub String);

/// The session side of the `codemode` tool.
#[async_trait::async_trait]
pub trait CodemodeHost: Send + Sync {
    /// `codemode.mode` (`getMode`, `index.ts:22-24`).
    fn mode(&self) -> CodemodeMode;

    /// `codemode.inlineBudget` in estimated tokens (`getInlineBudget`, `index.ts:26-29`), already
    /// defaulted to 3000.
    fn inline_budget(&self) -> f64;

    /// The tools a script may call (`ctx.tools`): the active `direct` tools and every `codemode` or
    /// `deferred` one, still including `codemode` itself; the tool leaves it out
    /// ([`super::description::callable_tools`]).
    fn callable_tools(&self) -> Vec<Arc<dyn Tool>>;

    /// Run a call the tool call `caller` made, through the agent's tool pipeline with the session's
    /// hooks (`ctx.executeTool(name, args, { signal })`, `execute.ts:343`). `args` is what the script
    /// passed; `Null` is `undefined`.
    async fn execute_nested(
        &self,
        caller: &ToolCallId,
        name: &str,
        args: Value,
        cancel: CancelToken,
    ) -> NestedOutcome;

    /// The `custom` entries on the current branch, from the root (`ctx.sessionManager.getBranch()`
    /// narrowed to `custom` entries).
    async fn branch_custom_entries(&self) -> Vec<BranchCustomEntry>;

    /// Append `data` as a `codemode-store` custom entry (`options.appendEntry`).
    ///
    /// # Errors
    ///
    /// [`StoreAppendFailed`] when the session could not persist the entry.
    async fn append_store_entry(
        &self,
        data: CodemodeStoreEntryData,
    ) -> Result<(), StoreAppendFailed>;

    /// Whether this host has model access, which is whether `models` is declared. Cheap: it is
    /// asked on every loadout resolution, where [`Self::models`] may have to compose a registry.
    fn has_model_access(&self) -> bool;

    /// The `models` namespace's backing (`ctx.modelRegistry`), or `None` when this host has no
    /// model access, in which case `models` is not declared (`models: false`, `tool.ts:78`).
    fn models(&self) -> Option<Arc<dyn CodemodeModels>>;
}

/// Where the tool finds its session. The session fills it when it is built; the extension, which
/// outlives any one session, and the tool it registers share it.
///
/// Empty means "no session context", which is upstream's plain-`Agent` case: scripts cannot call
/// tools, `store()` starts empty and writes are dropped (`execute.ts:311-312`).
#[derive(Clone, Default)]
pub struct CodemodeHostSlot {
    host: Arc<Mutex<Option<Arc<dyn CodemodeHost>>>>,
}

impl CodemodeHostSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Make `host` the session this slot's tool runs in. A later bind replaces it.
    pub fn bind(&self, host: Arc<dyn CodemodeHost>) {
        *self.host.lock().unwrap_or_else(PoisonError::into_inner) = Some(host);
    }

    #[must_use]
    pub fn current(&self) -> Option<Arc<dyn CodemodeHost>> {
        self.host
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl std::fmt::Debug for CodemodeHostSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodemodeHostSlot")
            .field("bound", &self.current().is_some())
            .finish()
    }
}
