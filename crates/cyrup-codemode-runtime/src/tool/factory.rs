//! Where the tool gets a sandbox from.
//!
//! The tool never names an engine: each script asks a [`SandboxFactory`] for a fresh
//! [`ScriptSandbox`] configured with that script's tools, globals and deadline (upstream builds a
//! `new CodemodeSandbox({...})` per call, `execute.ts:366-379` @v1.0.1). Tests inject a scripted
//! one; production injects the engine-bound `CodemodeSandbox`.
//!
//! # Production call path
//!
//! [`super::execute::execute_codemode`] calls [`SandboxFactory::create`] once per script.
//! [`UnavailableSandboxFactory`] is what a build without an engine uses: every script fails with
//! the named [`SandboxUnavailable::NotLinked`], never silently.

use std::sync::Arc;

use crate::types::{SandboxOptions, ScriptSandbox};

/// A sandbox could not be created.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SandboxUnavailable {
    /// This build links no JavaScript engine.
    #[error(
        "codemode sandbox unavailable: this build does not link the JavaScript engine that runs scripts"
    )]
    NotLinked,
    /// The engine is linked but refused to start.
    #[error("codemode sandbox failed to start: {0}")]
    Failed(String),
}

/// Creates the sandbox one script runs in.
pub trait SandboxFactory: Send + Sync {
    /// # Errors
    ///
    /// [`SandboxUnavailable`] when no sandbox can run the script.
    fn create(&self, options: SandboxOptions)
    -> Result<Arc<dyn ScriptSandbox>, SandboxUnavailable>;
}

/// The factory of a build without an engine: refuses every script with
/// [`SandboxUnavailable::NotLinked`].
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableSandboxFactory;

impl SandboxFactory for UnavailableSandboxFactory {
    fn create(
        &self,
        _options: SandboxOptions,
    ) -> Result<Arc<dyn ScriptSandbox>, SandboxUnavailable> {
        Err(SandboxUnavailable::NotLinked)
    }
}
