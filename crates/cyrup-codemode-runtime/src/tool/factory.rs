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
//! [`IsolatedSandboxFactory`] is the production factory: each script runs in a sandbox process of
//! its own. [`EngineSandboxFactory`] runs the isolate on a thread of this process, for tests and
//! embedders that cannot re-execute themselves. [`UnavailableSandboxFactory`] is what a build
//! without an engine uses: every script fails with the named [`SandboxUnavailable::NotLinked`],
//! never silently.

use std::sync::Arc;

use crate::sandbox::{CodemodeSandbox, HostCommand, Isolation};
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

/// Each script runs in a [`CodemodeSandbox`] (a V8 isolate,
/// ADR-0031) on a thread of this process, configured by what the tool registered. A script that
/// makes the engine abort takes the process with it; production uses [`IsolatedSandboxFactory`].
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineSandboxFactory;

impl SandboxFactory for EngineSandboxFactory {
    fn create(
        &self,
        options: SandboxOptions,
    ) -> Result<Arc<dyn ScriptSandbox>, SandboxUnavailable> {
        CodemodeSandbox::new(options)
            .map(|sandbox| Arc::new(sandbox) as Arc<dyn ScriptSandbox>)
            .map_err(|error| SandboxUnavailable::Failed(error.to_string()))
    }
}

/// The production factory: each script's isolate runs in a process of its own, started from
/// `command`, which must run [`run_sandbox_process`](crate::sandbox::run_sandbox_process). A script
/// that exhausts memory, crashes the engine or ignores its deadline costs the host that one
/// process and comes back as a failed script.
#[derive(Clone, Debug)]
pub struct IsolatedSandboxFactory {
    /// Why there is no command, when the running executable is unknown.
    command: Result<HostCommand, String>,
}

impl IsolatedSandboxFactory {
    #[must_use]
    pub fn new(command: HostCommand) -> Self {
        Self {
            command: Ok(command),
        }
    }

    /// Runs the sandbox processes from the executable that is running now. If the operating
    /// system cannot say which that is, every script is refused with that reason: running it in
    /// this process instead is what the factory exists to avoid.
    #[must_use]
    pub fn current_exe() -> Self {
        Self {
            command: HostCommand::current_exe().map_err(|error| {
                format!("the sandbox process cannot be started: the running executable is unknown ({error})")
            }),
        }
    }
}

impl SandboxFactory for IsolatedSandboxFactory {
    fn create(
        &self,
        options: SandboxOptions,
    ) -> Result<Arc<dyn ScriptSandbox>, SandboxUnavailable> {
        let command = self.command.clone().map_err(SandboxUnavailable::Failed)?;
        CodemodeSandbox::with_isolation(options, Isolation::Process(command))
            .map(|sandbox| Arc::new(sandbox) as Arc<dyn ScriptSandbox>)
            .map_err(|error| SandboxUnavailable::Failed(error.to_string()))
    }
}
