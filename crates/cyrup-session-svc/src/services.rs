//! `AgentSessionServices` — the cwd-bound subsystems the facade assembles and the session owns
//! (arch-11 §3.3). Recreated per session by the builder; exposed read-only so front-ends can
//! inspect the wired stack (settings, auth, resources, the extension host) without re-deriving it.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_config::{AuthStore, SettingsManager};
use cyrup_ext::ExtensionHost;
use cyrup_provider::Model;
use cyrup_resources::ResourceRegistry;
use cyrup_session::prompt::ContextStore;

/// Everything bound to a single cwd / session (arch-11 §3.3).
pub struct AgentSessionServices {
    pub cwd: PathBuf,
    /// Layered settings (global ◁ project ◁ cli), reflecting the resolved trust decision.
    pub settings: SettingsManager,
    /// Whether the project scope is trusted (gates project settings + post-trust resources).
    pub project_trusted: bool,
    /// Credential store (request-time auth resolution lives in `cyrup-config`).
    pub auth: Arc<AuthStore>,
    /// Discovered resources snapshot (skills / prompts / themes).
    pub resources: Arc<ResourceRegistry>,
    /// Session-scoped context cache (context files + skill pointers).
    pub context: Arc<ContextStore>,
    /// The extension host with native built-ins loaded; both seams are wired to the agent.
    pub ext_host: Arc<ExtensionHost>,
    /// The resolved active model for this session.
    pub model: Model,
    /// The assembled system prompt for this session (arch-06).
    pub system_prompt: String,
}
