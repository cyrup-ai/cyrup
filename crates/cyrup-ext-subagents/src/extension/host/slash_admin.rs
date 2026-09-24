//! `/subagents` — the admin surface (pi `src/slash/subagents-admin.ts`, 460 lines, registered at
//! `slash-commands.ts:869-875`).
//!
//! **Signature frozen by the orchestrator; the body is this batch's work.** `has_ui` is already
//! threaded through [`crate::extension::host::SubagentsExtension::dispatch_slash`], so the
//! no-UI branch (upstream's `metadataFor` text dump) and the interactive branch (upstream's
//! `selectAgent` → `chooseModel`/`chooseThinking`/`editSystemPrompt` → `persistSettingsField`
//! loop) both hang off this one entry point, exactly as `openSubagentsAdmin` does.
//!
//! The port itself is [`crate::registration::subagents_admin`]; this file is only the seam that
//! assembles pi's `(pi, ctx)` pair out of cyrup's parts — the executor's discovery config (pi's
//! `discoverAgentsAll(ctx.cwd)` plus its runtime-agent merge), the bound capability backend (pi's
//! `ctx.ui` and `ctx.model`), and the [`crate::discovery::EXTRA_AGENT_DIRS_ENV_VAR`] entries that
//! `isReadOnlyExtraAgent` (`subagents-admin.ts:139`) reads off `process.env` — taken from the SAME
//! [`crate::paths::Roots`] the discovery config was built from, so the agents the admin treats as
//! read-only extra-dir agents are exactly the ones discovery loaded from those dirs.

use std::path::Path;

use crate::error::SubagentError;
use crate::extension::host::SubagentsExtension;
use crate::registration::subagents_admin::{AdminContext, open_subagents_admin};

impl SubagentsExtension {
    /// pi `openSubagentsAdmin(pi, ctx, args)` (`subagents-admin.ts:396`).
    pub(crate) async fn slash_subagents(
        &self,
        args: &str,
        cwd: &Path,
        has_ui: bool,
    ) -> Result<String, SubagentError> {
        let config = self.executor.config_snapshot().await;
        // pi's `discoverAgentsAll(ctx.cwd)` — the SAME config every other surface in this crate
        // discovers through (`SubagentExecutor::discovery_config`), so `/subagents` can never see
        // a different agent set from the `subagent` tool's own `list`/`get` (R-SA-130). It also
        // carries the executor's runtime-agent registry, which is pi's `mergeRuntimeAgents(pi, …)`
        // at `subagents-admin.ts:37`.
        let cfg = self.executor.discovery_config(cwd, &config.roots)?;
        let services = self.executor.host_services();
        let ctx = AdminContext {
            cfg: &cfg,
            cwd,
            // pi's `ctx.ui` + `ctx.model`. `None` (no capability backend bound) makes the run take
            // the no-UI text path — see `AdminContext::interactive`.
            services: services.as_deref(),
            has_ui,
            // pi reads `process.env[EXTRA_AGENT_DIRS_ENV]` directly (`:140`), the same variable its
            // discovery reads (`agents.ts:2316`). Here both read the one resolved answer: the
            // `roots` `cfg` was just built from (`discovery_dirs_config` prepends exactly these),
            // so an extra-dir agent is read-only here iff discovery loaded it from an extra dir.
            extra_agent_dirs: config.roots.extra_agent_dirs().to_vec(),
        };
        open_subagents_admin(&ctx, args).await
    }
}
