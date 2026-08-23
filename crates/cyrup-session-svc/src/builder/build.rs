//! [`SessionBuilder::build`] — the ordered walk over `steps/`, and the final assembly.
//!
//! Every numbered step this used to inline now lives in [`super::steps`]; what is left here is the
//! order they run in, the handful of values that only exist between two of them, and step 10, which
//! stays inline because it is one struct literal fed by 27 bindings from every earlier step.

use std::sync::Arc;

use cyrup_core::{CancelToken, RunCancel};
use cyrup_config::AuthStore;
use tokio::sync::Mutex as AsyncMutex;

use super::steps::{
    self, AgentLoop, AgentParams, BuildCtx, ExtStack, ModelPick, PromptParams, PromptSurface,
    Resources, SessionTree, SettingsTrust, SubscriberWiring, ToolSurface, TrustSeams,
};
use super::SessionBuilder;
use crate::error::SessionServiceError;
use crate::services::AgentSessionServices;
use crate::session::AgentSession;
use crate::subscriber::Fanout;

impl SessionBuilder {
    /// Assemble the wired [`AgentSession`] (arch-11 §3.3). Async: discovery + context load + native
    /// extension `init` run here.
    pub async fn build(self) -> Result<AgentSession, SessionServiceError> {
        // Unpacked once, up front: every seam below is moved into exactly one step, and naming
        // them here is what lets the steps take owned values instead of `&self` (a shared
        // reference to the whole builder is not `Send`, so it could not cross a step's `await`).
        let Self {
            provider,
            config: cfg,
            settings_store,
            auth,
            native_extensions,
            cli_settings,
            prebuilt_manager,
            provider_resolver,
            stream_fn,
            key_resolver,
            skills_override,
            context_files_override,
            trust_store,
            trust_prompt,
        } = self;
        let cwd = cfg.cwd.clone();
        let SettingsTrust { settings, trusted } = steps::settings_and_trust(
            &cfg,
            &cwd,
            TrustSeams {
                settings_store: &settings_store,
                native_extensions: &native_extensions,
                trust_store: trust_store.as_ref(),
                trust_prompt: trust_prompt.as_ref(),
                cli_settings: &cli_settings,
            },
        )
        .await;
        let ctx =
            BuildCtx { cfg, cwd, cancel: RunCancel::new(), provider, settings, trusted };

        // ---- 2. auth (cyrup-config) ------------------------------------------------------------
        let auth =
            auth.unwrap_or_else(|| Arc::new(AuthStore::at(ctx.cfg.agent_dir.join("auth.json"))));

        let mut tree = steps::open_session_tree(&ctx, prebuilt_manager)?;
        let model = steps::resolve_session_model(&ctx, &tree)?;
        let tools = steps::tool_registry(&ctx, &tree, &model);
        let ext = steps::extension_stack(&ctx, &tools, &model, native_extensions).await?;
        let Resources {
            resources,
            startup_diagnostics,
            model_config,
            catalog_overlay,
            guest_providers,
            skills,
        } = steps::discover_resources(&ctx, &ext, &tools, skills_override).await?;
        let PromptSurface { context_store, active_tools, system_prompt, dynamic_tools } =
            steps::context_and_prompt(
                &ctx,
                &ext,
                &tools,
                PromptParams { skills, context_files_override },
            )
            .await?;

        // The token both extension seams and the fan-out subscriber are cancelled by.
        let session_cancel = CancelToken::new();
        let AgentLoop { agent, handle, provider_swap, telemetry_enabled } = steps::agent_loop(
            &ctx,
            &ext,
            &model,
            AgentParams {
                system_prompt: system_prompt.clone(),
                active_tools,
                seed: steps::seed_transcript(&tree),
                session_id: tree.session_id.clone(),
                stream_fn,
                key_resolver,
                provider_resolver,
            },
        );

        steps::seed_session_entries(&mut tree, &model)?;
        let session_dir = steps::session_dir_of(&ctx, &tree);
        let SessionTree { manager, session_id, .. } = tree;
        let manager = Arc::new(AsyncMutex::new(manager));
        // Attach the live tree manager to the (already control-wired) host-services backend so a
        // loaded guest's `append_entry`/`set_session_name`/`set_label` capability mutates THIS
        // session's real tree (arch-08 §5.6; Pi appends synchronously, agent-session.ts:2265-2279).
        ext.host_services.attach_session(manager.clone());
        let fanout = Arc::new(Fanout::new());
        steps::wire_subscribers(
            &agent,
            &ext,
            SubscriberWiring {
                fanout: fanout.clone(),
                manager: manager.clone(),
                handle: handle.clone(),
                session_cancel: session_cancel.clone(),
            },
        );
        let agent = Arc::new(agent);

        // The 27 bindings the assembly reads, unpacked from the step outputs that carried them.
        let ToolSurface {
            shell,
            shell_path: shell_path_setting,
            shell_command_prefix: shell_command_prefix_setting,
            bash_proc,
            read_model_vision,
            bash_session_env,
            ..
        } = tools;
        let ExtStack { host_services, host: ext_host, .. } = ext;
        let ModelPick { resolved: resolved_model, model_ref, fallback_message, .. } = model;
        let BuildCtx { cfg, cwd, settings, .. } = ctx;

        // ---- 10. assemble the session --------------------------------------------------------
        // `host_services` (the concrete arch-08 §5.6 backend) was built + seeded + control-wired at
        // step 4a and injected into every wasm load; it is moved into the services bundle below so
        // `AgentSession::apply_pending_control` drains the SAME queue guest `control` ops reach (Pi
        // `createCommandContext`, agent-session.ts:1158).

        // Resolve the settings-driven knobs for the retry / auto-compaction subsystems BEFORE the
        // `settings` value is moved into the services bundle.
        let eff = settings.effective();
        let cfg_compaction = eff.compaction_settings();
        let to_u32 = |v: i64| u32::try_from(v.max(0)).unwrap_or(u32::MAX);
        let extras = crate::session::SessionExtras {
            telemetry_enabled,
            compaction_settings: cyrup_session::compaction::CompactionSettings {
                enabled: cfg_compaction.enabled,
                reserve_tokens: to_u32(cfg_compaction.reserve_tokens),
                keep_recent_tokens: to_u32(cfg_compaction.keep_recent_tokens),
            },
            branch_summary_settings: cyrup_session::compaction::BranchSummarySettings {
                reserve_tokens: to_u32(eff.branch_summary_reserve_tokens()),
                skip_prompt: eff.branch_summary_skip_prompt(),
            },
            auto_compaction_enabled: eff.compaction_enabled(),
            auto_retry_enabled: eff.retry_enabled(),
            retry_max_retries: to_u32(eff.retry_max_retries()),
            retry_base_delay_ms: u64::try_from(eff.retry_base_delay_ms().max(0)).unwrap_or(0),
            proc: bash_proc,
            shell,
            shell_path: shell_path_setting,
            shell_command_prefix: shell_command_prefix_setting,
            dynamic_tools,
            handle,
            bash_session_env,
            read_model_vision,
        };

        let services = AgentSessionServices {
            cwd,
            agent_dir: cfg.agent_dir.clone(),
            session_dir,
            home: cfg.home.clone(),
            settings,
            project_trusted: trusted,
            auth,
            resources,
            startup_diagnostics,
            model_config,
            catalog_overlay,
            context: context_store,
            ext_host,
            guest_providers,
            model: resolved_model,
            system_prompt,
            host_services,
            extension_flag_values: cfg.extension_flag_values.clone(),
        };

        Ok(AgentSession::from_parts(
            agent,
            manager,
            fanout,
            provider_swap,
            services,
            model_ref,
            session_cancel,
            session_id,
            fallback_message,
            extras,
        ))
    }
}
