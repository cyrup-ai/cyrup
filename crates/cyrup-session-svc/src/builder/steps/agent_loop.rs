//! Step 4 (tools + isolation + policy) and step 9 (the agent loop) — the tool surface the model
//! is given, and the [`Agent`] built over it.
//!
//! They live together because step 9 is the only consumer of step 4's tool set that is not itself
//! a resource step: the registry decides what `bash`/`read` can reach, and the agent loop is where
//! that set, the composed hooks and both extension seams become one running agent.

use std::sync::Arc;

use cyrup_agent::Agent;
use cyrup_core::{CancelToken, ModelRef};
use cyrup_ext::{EventKind, HostEvent};
use cyrup_session::manager::SessionManager;
use cyrup_tools::{
    Backend, BashOpts, ProcOps, ProtectedFs, ShellConfig, ToolRegistry, ToolsOptions, TraversalFs,
};
use tokio::sync::Mutex as AsyncMutex;

use super::{BuildCtx, ExtStack, ModelPick, SessionTree};
use crate::builder::settings_parse::apply_http_proxy_settings;
use crate::builder::tools::select_active_tools;
use crate::builder::{parse_queue_mode, parse_transport, thinking_level_to_str};
use crate::provider_swap::{ProviderResolver, ProviderSwap};
use crate::subscriber::{Fanout, SvcSubscriber};

/// The tool surface step 4 resolves: the shell + process seams both bash paths share, the two live
/// handles the session re-pushes on `/model`, and the three tool sets later steps read.
pub(in crate::builder) struct ToolSurface {
    pub(in crate::builder) shell: ShellConfig,
    pub(in crate::builder) shell_path: Option<String>,
    pub(in crate::builder) shell_command_prefix: Option<String>,
    /// The process backend the immediate-bash seam and the extension `exec` grant run against.
    pub(in crate::builder) bash_proc: Arc<dyn ProcOps>,
    pub(in crate::builder) read_model_vision: cyrup_tools::config::ModelVisionHandle,
    pub(in crate::builder) bash_session_env: cyrup_tools::config::SessionEnvHandle,
    /// Every `Availability`-visible tool — the enable-able set the dynamic registry is built from.
    pub(in crate::builder) visible: Vec<Arc<dyn cyrup_core::Tool>>,
    /// The build-time selection (`tools`/`noTools`/`excludeTools` applied over `visible`).
    pub(in crate::builder) base_tools: Vec<Arc<dyn cyrup_core::Tool>>,
    pub(in crate::builder) read_available: bool,
}

/// Step 4 — tools + isolation + policy (cyrup-tools).
pub(in crate::builder) fn tool_registry(
    ctx: &BuildCtx,
    tree: &SessionTree,
    model: &ModelPick,
) -> ToolSurface {
    let BuildCtx { cfg, cwd, settings, .. } = ctx;
    let session_id = &tree.session_id;
    let manager = &tree.manager;
    let resolved_model = &model.resolved;
    let model_ref = &model.model_ref;
    let thinking = model.thinking;
    // `shellPath`/`shellCommandPrefix` settings (Pi `getShellPath`/`getShellCommandPrefix`,
    // settings-manager.ts:864-865,895-896), read once here and threaded into BOTH bash seams:
    // the agent-loop `bash` tool (via `ToolsOptions.bash` below, matching Pi's `_buildRuntime`
    // passing `{commandPrefix, shellPath}` into `createAllToolDefinitions`, agent-session.ts:
    // 2436-2448) and the immediate-bash RPC seam (via `SessionExtras` below, matching Pi's
    // `executeBash` re-reading the same two settings, agent-session.ts:2624-2632).
    let shell_path_setting = settings.effective().shell_path();
    let shell_command_prefix_setting = settings.effective().shell_command_prefix();
    let shell = ShellConfig::detect();
    let base = Backend::local(shell.clone());
    // The process backend the immediate-bash seam (#8) runs against (kept past `base`'s move).
    let bash_proc = base.proc.clone();
    let mut fs = base.fs.clone();
    if cfg.confine_to_cwd {
        fs = Arc::new(TraversalFs::new(fs, cwd.clone()));
    }
    if cfg.protect_paths {
        fs = Arc::new(ProtectedFs::with_defaults(fs));
    }
    let backend = Backend { fs, proc: base.proc.clone() };
    // The live session metadata every `bash` child gets as `CYRUP_*` (Pi's `resolveSpawnContext`
    // reads the same five values off the per-call `ExtensionContext`, bash.ts:171-181). Pi's
    // values are "resolved when each command starts" (docs/environment-variables.md:27), so this
    // is a shared HANDLE the session mutates on `set_model` / `set_thinking_level`, never a
    // snapshot baked into the tool.
    // `read`'s non-vision-model warning (pi `tools/read.ts`): the handle is seeded from the
    // RESOLVED model's declared input modalities and re-pushed on every `/model` switch, exactly
    // as `bash_session_env` carries provider/model. Without this the tool's
    // `ReadOpts::model_vision` stayed `None` and `supports_images_now()` fell back to `true`,
    // so the warning was unreachable and an image handed to a text-only model produced a
    // provider error instead of the tool's own diagnostic.
    // A modelless session (SEAM-075) has no declared modalities to seed from; `read` then keeps
    // its `supports_images_now()` default until the first `/model` re-pushes the real value.
    let read_model_vision = cyrup_tools::config::ModelVisionHandle::new(
        resolved_model.as_ref().is_none_or(cyrup_provider::Model::supports_image_input),
    );
    let bash_session_env = cyrup_tools::config::SessionEnvHandle::new(
        cyrup_tools::config::SessionEnvInfo {
            session_id: Some(session_id.to_string()),
            // `None` for an ephemeral/in-memory session — Pi leaves `PI_SESSION_FILE` unset
            // rather than empty in that case (bash.ts:173-174).
            session_file: manager.session_file().map(std::path::Path::to_path_buf),
            // Likewise `None` while the session has no model: pi resolves `CYRUP_PROVIDER`/
            // `CYRUP_MODEL` from `ctx.model` per command (bash.ts:171-181), so a modelless
            // session leaves them unset rather than exporting a placeholder.
            provider: model_ref.as_ref().map(|m| m.provider.to_string()),
            model: model_ref.as_ref().map(|m| m.model.to_string()),
            reasoning_level: Some(thinking_level_to_str(thinking)),
        },
    );
    let registry = ToolRegistry::with_builtins(
        cwd.clone(),
        backend,
        ToolsOptions {
            read: cyrup_tools::config::ReadOpts {
                model_vision: Some(read_model_vision.clone()),
                // `images.autoResize` (Pi `_buildRuntime`: `const autoResizeImages =
                // this.settingsManager.getImageAutoResize()` → `read: { autoResizeImages }`,
                // agent-session.ts:2553,2564). Without this the setting had no consumer at all
                // and `read` downsampled every image to 2000px regardless.
                auto_resize_images: settings.effective().image_auto_resize(),
                ..cyrup_tools::config::ReadOpts::default()
            },
            bash: BashOpts {
                command_prefix: shell_command_prefix_setting.clone(),
                shell_path: shell_path_setting.clone(),
                session_env: Some(bash_session_env.clone()),
                // pi `getShellEnv()` (`utils/shell.ts:122-134`) unconditionally prepends
                // `getBinDir()` to PATH for EVERY bash child (`tools/bash.ts:100,165`); there is
                // no pi path where the bash tool spawns without it.
                //
                // cyrup set this only on the user-facing `/bash` seam
                // (`session.rs:4225`, the same `<agent_dir>/bin`), leaving the agent-loop `bash`
                // tool — the one the MODEL calls — with `bin_dir: None`, which makes
                // `ops::shell::shell_env` return an empty overlay and inherit the parent PATH
                // unchanged. So a binary cyrup manages into `<agent_dir>/bin` produced
                // `command not found` for the model while the identical command succeeded
                // through `/bash`: two bash paths in one process disagreeing about PATH, which
                // reads as nondeterminism from the outside.
                bin_dir: Some(cfg.agent_dir.join("bin")),
                ..BashOpts::default()
            },
            ..ToolsOptions::default()
        },
    );
    let visible = registry.visible(&cfg.tool_availability);
    // Tool-set selection (Pi sdk.ts:244-251): an explicit `tools` allowlist, or `noTools`
    // ("all" ⇒ none; "builtin" ⇒ drop the default built-ins), then minus the `excludeTools`
    // denylist. Absent all three, the Availability-visible set is kept verbatim.
    let base_tools = select_active_tools(&visible, cfg);
    let read_available = base_tools.iter().any(|t| t.name() == "read");

    ToolSurface {
        shell,
        shell_path: shell_path_setting,
        shell_command_prefix: shell_command_prefix_setting,
        bash_proc,
        read_model_vision,
        bash_session_env,
        visible,
        base_tools,
        read_available,
    }
}

/// What step 9 hands back: the built (not yet subscribed) agent, the shared self-handle the
/// subscribers and hooks capture, the swappable stream source `/model` mutates, and the telemetry
/// verdict the assembly carries onto [`crate::session::SessionExtras`].
pub(in crate::builder) struct AgentLoop {
    pub(in crate::builder) agent: Agent,
    pub(in crate::builder) handle: Arc<crate::session::SessionHandle>,
    pub(in crate::builder) provider_swap: Arc<ProviderSwap>,
    pub(in crate::builder) telemetry_enabled: bool,
}

/// The per-run values step 9 consumes outright: the prompt + tool set it was handed, the resumed
/// transcript, and the four embedder seams that only the agent loop reads.
pub(in crate::builder) struct AgentParams {
    pub(in crate::builder) system_prompt: String,
    pub(in crate::builder) active_tools: Vec<Arc<dyn cyrup_core::Tool>>,
    pub(in crate::builder) seed: Vec<cyrup_agent::AgentMessage>,
    pub(in crate::builder) session_id: cyrup_core::SessionId,
    pub(in crate::builder) stream_fn: Option<Arc<dyn cyrup_agent::StreamFn>>,
    pub(in crate::builder) key_resolver: Option<Arc<dyn cyrup_agent::ApiKeyResolver>>,
    pub(in crate::builder) provider_resolver: Option<Arc<dyn ProviderResolver>>,
}

/// Step 9 — agent loop: provider + tools + composed hooks + both extension seams.
pub(in crate::builder) fn agent_loop(
    ctx: &BuildCtx,
    ext: &ExtStack,
    model: &ModelPick,
    p: AgentParams,
) -> AgentLoop {
    let BuildCtx { cfg, settings, provider, .. } = ctx;
    let AgentParams {
        system_prompt,
        active_tools,
        seed,
        session_id,
        stream_fn: custom_stream_fn,
        key_resolver: custom_key_resolver,
        provider_resolver,
    } = p;
    let ext_host = &ext.host;
    let has_ui = ext.has_ui;
    // Step 8's hooks seam — the host itself was built at step 4b.
    let ext_hooks = ext_host.hooks();
    let resolved_model = &model.resolved;
    let model_ref = &model.model_ref;
    let thinking = model.thinking;
    // `blockImages` defense-in-depth (Pi sdk.ts:254-289): the convert-to-llm seam strips image
    // content when the setting is on, deduping consecutive placeholders. Folded into PolicyHooks
    // so it rides the agent's single `convertToLlm` slot.
    let block_images = settings.effective().block_images();
    // The shared self-handle: bound to the owning `Arc<AgentSession>` by `into_shared`, and read
    // by the persist+fan-out subscriber (`_handleAgentEvent`), the post-run driver, and — since
    // the turn-boundary tool refresh — `PolicyHooks::prepare_next_turn`. Declared HERE, ahead of
    // the hooks, because the hooks are built before the session and must capture it; it is an
    // empty `OnceLock` either way, so nothing observable moved with it.
    let handle = Arc::new(crate::session::SessionHandle::default());
    let policy_hooks = Arc::new(crate::hooks::PolicyHooks::new(
        cfg.permission_policy.clone(),
        ext_hooks,
        has_ui,
        block_images,
        handle.clone(),
    ));
    let eff = settings.effective();
    // Provider attribution + opencode session headers (Pi sdk.ts:323-330, #20). Telemetry is the
    // env override (`CYRUP_TELEMETRY`/`PI_TELEMETRY`) else the `enableInstallTelemetry` setting.
    let env = cyrup_config::EnvVars::from_process();
    let telemetry_enabled = env.telemetry.unwrap_or_else(|| eff.enable_install_telemetry());
    // No model ⇒ no provider to attribute to; the headers are recomputed on the first
    // `/model` anyway (`apply_model_change`).
    let attribution_headers = resolved_model.as_ref().and_then(|m| {
        crate::attribution::merge_provider_attribution_headers(
            m,
            telemetry_enabled,
            Some(&session_id),
            &[],
        )
    });
    // The swappable stream source the agent loop streams through: it wraps the resolved provider
    // and the (optional) resolver seam so a cross-provider `/model` select can install a new
    // provider in place without rebuilding the agent (Pi live model+provider switch). The SAME
    // `Arc` is handed to the agent (as its `StreamFn`) and to the session (to mutate on select).
    let provider_swap =
        Arc::new(ProviderSwap::new(provider.clone(), provider_resolver));
    // Transport selection (Pi `AgentOptions.streamFn`, sdk.ts:301): an embedder-supplied custom
    // `StreamFn` (e.g. `ProxyStreamFn`) becomes THE transport the agent loop streams through;
    // absent one, the provider-backed `ProviderSwap` is used (the default live-swappable path).
    let agent_stream_fn: Arc<dyn cyrup_agent::StreamFn> = match custom_stream_fn {
        Some(f) => f,
        None => provider_swap.clone(),
    };
    // SEAM-075 — the agent's run baseline. pi's agent holds `Model | undefined`
    // (`AgentSession.model` is a straight read of `this.agent.state.model`,
    // agent-session.ts:866-868), so a modelless session builds an agent with no model at all.
    // `cyrup_agent::StateInner::model` is a non-optional `ModelRef`, so it is seeded with an
    // EMPTY address here. That value is unreachable while the session is modelless: every path
    // into `Agent::run` goes through [`AgentSession::prompt`] → `prepare_and_assemble`, which
    // returns [`SessionServiceError::NoModelSelected`] before touching the agent (pi
    // agent-session.ts:1178-1180), and the first `/model` overwrites it through
    // `agent.set_model`. It is NOT a catalog entry and no reader sees it — the `/model` picker,
    // the footer, `state_view`, the attribution headers and the `CYRUP_*` env all read the
    // session's `Option<ModelRef>`, which stays `None` until a model is selected.
    let agent_model = model_ref.clone().unwrap_or_else(|| ModelRef {
        provider: cyrup_core::ProviderId::from(""),
        api: None,
        model: cyrup_core::ModelId::from(""),
    });
    let mut agent_builder = Agent::builder(agent_model, agent_stream_fn)
    .system_prompt(system_prompt)
    .thinking_level(thinking)
    .tools(active_tools)
    .messages(seed)
    .hooks(policy_hooks)
    .session_id(session_id.clone())
    // Settings→Agent wiring (Pi sdk.ts:356-360): queue modes + transport + custom thinking budgets.
    .steering_mode(parse_queue_mode(&eff.steering_mode()))
    .follow_up_mode(parse_queue_mode(&eff.follow_up_mode()))
    // `transport` (Pi sdk.ts:357 `transport: settingsManager.getTransport()`). The setting was
    // parsed, migrated from the legacy `websockets` boolean and offered in the `/settings` grid,
    // but never reached the agent — so `AgentBuilder::transport` had no non-test caller and the
    // value died in the config layer. It now rides `StreamOptions.transport` into every
    // `StreamFn::stream` call (agent.rs `gen_config.transport`), which is the seam an
    // embedder-supplied `StreamFn` (e.g. `ProxyStreamFn`) and every wire API read from.
    .transport(parse_transport(&eff.transport()));
    if let Some(h) = attribution_headers {
        agent_builder = agent_builder.headers(h);
    }
    if let Some(budgets) = eff.thinking_budgets() {
        // Map the config struct (`i64`) to the provider struct (`u64`); negatives clamp to 0.
        let to_u64 = |v: Option<i64>| v.map(|n| n.max(0) as u64);
        agent_builder = agent_builder.thinking_budgets(cyrup_provider::ThinkingBudgets {
            minimal: to_u64(budgets.minimal),
            low: to_u64(budgets.low),
            medium: to_u64(budgets.medium),
            high: to_u64(budgets.high),
        });
    }
    // HTTP proxy + idle-timeout from settings (Pi `applyHttpProxySettings(settings.httpProxy)` +
    // `configureHttpDispatcher(getHttpIdleTimeoutMs())`, main.ts:744-745). The `httpProxy` setting
    // becomes a provider-scoped `HTTP_PROXY`/`HTTPS_PROXY` overlay (Pi `StreamOptions.env`) that the
    // provider's proxy resolver honors; the idle timeout becomes the request `timeout_ms`. The
    // read is setting-only, mirroring Pi's `getGlobalSettings().httpProxy`; the ambient-wins half
    // of pi's `??=` lives in `node_http_proxy::get_proxy_env` (CFG-060, which deleted the
    // accessor's `EnvVars` argument — this call passed `EnvVars::default()` to defeat it).
    if let Some(overlay) = apply_http_proxy_settings(eff.http_proxy()) {
        agent_builder = agent_builder.provider_env(overlay);
    }
    // PROV-006. Pi's `configureHttpDispatcher(getHttpIdleTimeoutMs())` installs a PROCESS-GLOBAL
    // dispatcher (main.ts:802, interactive-mode.ts:1778) that bounds every outbound HTTP request
    // — provider streams, catalog refreshes, everything — so the equivalent global is installed
    // here, not just threaded onto this agent's requests.
    //
    // `0` is passed through rather than skipped: `httpIdleTimeoutMs: 0` / `"disabled"` means the
    // user turned the timeout OFF, and dropping the call would silently leave the previous value
    // (or the 5-minute default) in place. The old `timeout_ms > 0` guard did exactly that.
    if let Ok(timeout_ms) = eff.http_idle_timeout_ms() {
        cyrup_provider::configure_http_idle_timeout(timeout_ms);
        agent_builder = agent_builder.timeout_ms(timeout_ms);
    }

    // `settings.retry.provider.*` — Pi's `getProviderRetrySettings()`, applied in `sdk.ts`'s
    // `streamFn` as `options?.X ?? providerRetrySettings.X` (sdk.ts:303-317). `timeoutMs` wins
    // over `httpIdleTimeoutMs` when set, which is why it is applied after the block above.
    // Negative values (JSON has no unsigned type) are treated as unset rather than clamped to 0,
    // since `0` is a meaningful "disabled" for the timeout and "no retries" for the budget.
    {
        let retry = eff.provider_retry_settings();
        if let Some(timeout_ms) = retry.timeout_ms.filter(|ms| *ms >= 0) {
            agent_builder = agent_builder.timeout_ms(timeout_ms as u64);
        }
        if let Some(max_retries) = retry.max_retries.filter(|n| *n >= 0) {
            agent_builder = agent_builder.max_retries(max_retries as u32);
        }
        if retry.max_retry_delay_ms >= 0 {
            agent_builder = agent_builder.max_retry_delay_ms(retry.max_retry_delay_ms as u64);
        }
    }

    // CFG-006 / AGENT-031 — `websocketConnectTimeoutMs`. pi's `streamFn` resolves it as
    // `options?.websocketConnectTimeoutMs ?? settingsManager.getWebSocketConnectTimeoutMs()`
    // (`core/sdk.ts:310-311` @v0.83.0) and spreads it onto every `streamSimple` call.
    //
    // Both halves existed and neither was connected: `Settings::websocket_connect_timeout_ms`
    // parsed and validated the key, `AgentBuilder::websocket_connect_timeout_ms` threaded it to
    // `StreamOptions` (`cyrup-provider/src/stream.rs:201`) — and NOTHING assigned it, so a user
    // who set the key got no error and no effect. Deliberately NOT applied in the retry block
    // above: it is a separate pi rung with its own settings getter, and (unlike `timeoutMs`) no
    // `retry.provider.*` value overrides it.
    //
    // A parse error is dropped exactly as the `http_idle_timeout_ms` rung above drops one — the
    // invalid-setting diagnostic is the settings layer's, not this builder's.
    if let Ok(Some(ms)) = eff.websocket_connect_timeout_ms() {
        agent_builder = agent_builder.websocket_connect_timeout_ms(ms);
    }

    // gap-08 #2/#3: install the provider transport extension seams. `on_payload` routes the
    // outbound body through the tested `before_provider_request` [mutate] facade (Pi
    // `emitBeforeProviderRequest` in sdk.ts onPayload, :332-338); `on_response` constructs the
    // previously-NOWHERE `HostEvent::AfterProviderResponse` notify ({status, headers}, Pi
    // sdk.ts:339-348). Both are gated on a live subscriber so the common no-extension path pays
    // nothing. The dispatch is async (wasm) — hence the async hook signatures (no block_on).
    {
        let h = ext_host.clone();
        agent_builder = agent_builder.on_payload(Arc::new(move |payload, _model| {
            let h = h.clone();
            Box::pin(async move {
                if h.dispatcher().no_subscribers(EventKind::BeforeProviderRequest) {
                    return None;
                }
                let out =
                    h.emit_before_provider_request(payload.clone(), &CancelToken::new()).await;
                (out != payload).then_some(out)
            })
        }));
        let h = ext_host.clone();
        agent_builder = agent_builder.on_response(Arc::new(move |resp, _model| {
            let h = h.clone();
            Box::pin(async move {
                if h.dispatcher().no_subscribers(EventKind::AfterProviderResponse) {
                    return;
                }
                let headers = serde_json::to_value(&resp.headers).unwrap_or_default();
                h.dispatcher()
                    .dispatch_notify(
                        &HostEvent::AfterProviderResponse {
                            status: u32::from(resp.status),
                            headers,
                        },
                        &CancelToken::new(),
                    )
                    .await;
            })
        }));
    }

    // Dynamic per-request key resolution (Pi key resolver): consulted on every turn, overriding
    // any static key. Threaded whether or not a custom transport is installed.
    if let Some(kr) = custom_key_resolver {
        agent_builder = agent_builder.key_resolver(kr);
    }

    let agent = agent_builder.build();

    AgentLoop { agent, handle, provider_swap, telemetry_enabled }
}

/// The four handles the agent's two subscribers close over.
pub(in crate::builder) struct SubscriberWiring {
    pub(in crate::builder) fanout: Arc<Fanout>,
    pub(in crate::builder) manager: Arc<AsyncMutex<SessionManager>>,
    pub(in crate::builder) handle: Arc<crate::session::SessionHandle>,
    pub(in crate::builder) session_cancel: CancelToken,
}

/// Attach the extension notify seam (step 8), then the facade's persist+fan-out subscriber.
pub(in crate::builder) fn wire_subscribers(agent: &Agent, ext: &ExtStack, w: SubscriberWiring) {
    agent.subscribe(ext.host.subscriber(w.session_cancel.clone()));
    agent.subscribe(Arc::new(SvcSubscriber::new(
        w.fanout,
        w.manager,
        w.handle,
        ext.host.clone(),
        w.session_cancel,
    )));
}
