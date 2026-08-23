//! Step 6 — the context store and the system prompt (cyrup-session arch-06).
//!
//! Loads the `AGENTS.md`/`CLAUDE.md` set under the trust gate, merges the extension-contributed
//! tools over the build-time selection so an extension tool's prompt text actually reaches the
//! model, assembles the system prompt, and stands up the dynamic tool registry the guest
//! `setActiveTools` capability and the CLI toggle both mutate.

use std::path::Path;
use std::sync::Arc;

use cyrup_resources::SkillPointer;
use cyrup_session::prompt::{
    ContextFileLoader, ContextSnapshot, DocsPointers, PromptInputs, ResolvedOverride,
    SystemPromptBuilder, ToolPromptContribution,
};

use super::{BuildCtx, ExtStack, ToolSurface};
use crate::builder::tool_contribution;
use crate::error::SessionServiceError;

/// The two per-run values step 6 consumes outright.
pub(in crate::builder) struct PromptParams {
    /// The read-gated skill pointers step 5 derived (already `skillsOverride`-transformed).
    pub(in crate::builder) skills: Vec<SkillPointer>,
    pub(in crate::builder) context_files_override: Option<crate::builder::ContextFilesOverrideFn>,
}

/// What step 6 hands back.
pub(in crate::builder) struct PromptSurface {
    pub(in crate::builder) context_store: Arc<cyrup_session::prompt::ContextStore>,
    /// pi's `getActiveTools()`: the build-time selection with the extension additions/overrides
    /// merged over it — what the agent loop is actually given.
    pub(in crate::builder) active_tools: Vec<Arc<dyn cyrup_core::Tool>>,
    pub(in crate::builder) system_prompt: String,
    pub(in crate::builder) dynamic_tools: Arc<std::sync::Mutex<crate::tools::DynamicToolState>>,
}

/// Step 6 — context store + system prompt.
pub(in crate::builder) async fn context_and_prompt(
    ctx: &BuildCtx,
    ext: &ExtStack,
    tools: &ToolSurface,
    p: PromptParams,
) -> Result<PromptSurface, SessionServiceError> {
    let BuildCtx { cfg, cwd, cancel, settings, .. } = ctx;
    let trusted = ctx.trusted;
    let ext_host = &ext.host;
    let host_services = &ext.host_services;
    let visible = &tools.visible;
    let base_tools = &tools.base_tools;
    let PromptParams { skills, context_files_override } = p;
    let loader = ContextFileLoader::new(
        cwd.clone(),
        cfg.agent_dir.clone(),
        trusted,
        cfg.no_context_files,
    );
    let context_store = Arc::new(cyrup_session::prompt::ContextStore::new());
    context_store
        .reload(cancel, loader, Arc::from(skills), ResolvedOverride::default())
        .await?;
    // Synthetic context-file injection (Pi `agentsFilesOverride`, resource-loader.ts:474):
    // transform the loaded `AGENTS.md`/`CLAUDE.md` set before the system prompt reads it.
    if let Some(f) = context_files_override {
        let snap = context_store.snapshot();
        let files = f(snap.context_files.to_vec());
        context_store.store(ContextSnapshot {
            context_files: Arc::from(files),
            skills: snap.skills.clone(),
            override_source: snap.override_source.clone(),
            diagnostics: snap.diagnostics.clone(),
        });
    }
    let snapshot = context_store.snapshot();

    // The extension-shaped active tool set (Pi `pi.getActiveTools()` after the extension
    // `active_tools` merge): base build-time selection PLUS any extension additions/overrides
    // (e.g. a native extension that overrides a built-in `bash`).
    //
    // EXT-038 — this used to be computed AFTER the prompt was built, so the prompt was derived
    // from `base_tools` alone. Upstream builds `_toolPromptSnippets` / `_toolPromptGuidelines`
    // from `definitionRegistry`, which is the base definitions with `allCustomTools` (the
    // extension-contributed ones) merged over them (`agent-session.ts:2471-2504` @v0.83.0) —
    // so an extension tool's `promptSnippet`/`promptGuidelines` DO reach the system prompt, and
    // an extension OVERRIDE of a built-in contributes the override's text, not the built-in's.
    // In cyrup neither happened: a guest could register a fully-described tool and the model
    // was never told it existed.
    let active_tools = ext_host.active_tools(base_tools)?;

    // pi's `definitionRegistry`: base first, then each custom/extension tool `set` over it by
    // NAME — so an override replaces the built-in's entry rather than adding a second one, and
    // the base order is preserved for everything not overridden (`:2471-2487`).
    let prompt_tools: Vec<Arc<dyn cyrup_core::Tool>> = {
        let mut order: Vec<Arc<dyn cyrup_core::Tool>> = base_tools.clone();
        for t in active_tools.iter() {
            // `iter_mut().find()` rather than `position()` + `order[i]`: same `set`-by-name
            // semantics, without the raw index the workspace denies (`indexing_slicing`).
            if let Some(slot) = order.iter_mut().find(|b| b.name() == t.name()) {
                *slot = t.clone();
            } else {
                order.push(t.clone());
            }
        }
        order
    };
    let selected_tools: Vec<Arc<str>> =
        prompt_tools.iter().map(|t| Arc::from(t.name())).collect();
    let tool_contributions: Vec<ToolPromptContribution> =
        prompt_tools.iter().map(tool_contribution).collect();

    // CFG-035 — `SYSTEM.md` / `APPEND_SYSTEM.md` discovery. Pi's `load()` resolves
    // `this.systemPromptSource ?? this.discoverSystemPromptFile()` and, for the append leg,
    // uses the CLI sources when present and otherwise the SINGLE discovered file
    // (`resource-loader.ts:525,531-535` @v0.83.0). Both discoverers are the same two rungs
    // (`:1022-1032`, `:1034-1044`): the project file under the trust gate, then the global one.
    //
    // Neither file was read at all before this: `.cyrup/SYSTEM.md` and `.cyrup/APPEND_SYSTEM.md`
    // were only TRUST-GATE MARKERS (`cyrup-config/src/trust.rs:208-209`), so cyrup asked the
    // user to trust a project because of a file it then never opened.
    //
    // The discoverer itself is `cyrup_resources`' — the SAME function whose result rides out on
    // `DiscoveryReport::{system_prompt_file, append_system_prompt_file}`. A private second copy
    // lived here until this pass; two copies of one upstream function is the `encode_cwd`
    // duplication hazard (two drift-free copies that were both wrong), and here the dead copy
    // was the one covered by tests while the live one was not.
    let discovered_system_prompt = cfg
        .system_prompt
        .is_none()
        .then(|| {
            cyrup_resources::discover_system_prompt_file(cwd, &cfg.agent_dir, trusted)
        })
        .flatten()
        .and_then(|p| read_discovered_prompt(&p, "system prompt"));
    // pi REPLACES rather than accumulates: `let appendSources = this.appendSystemPromptSource;
    // if (!appendSources) { …discovered… }` (`:531-535`) — a CLI `--append-system-prompt` means
    // the discovered file is not consulted, and discovery itself yields exactly ONE path.
    let discovered_append = cfg
        .append_system_prompt
        .is_none()
        .then(|| {
            cyrup_resources::discover_append_system_prompt_file(cwd, &cfg.agent_dir, trusted)
        })
        .flatten()
        .and_then(|p| read_discovered_prompt(&p, "append system prompt"));

    let prompt_inputs = PromptInputs {
        custom_prompt: cfg
            .system_prompt
            .clone()
            .or(discovered_system_prompt)
            .map(Arc::from),
        selected_tools: Some(selected_tools),
        tool_contributions,
        prompt_guidelines: Vec::new(),
        append_system_prompt: cfg
            .append_system_prompt
            .clone()
            .or(discovered_append)
            .map(Arc::from),
        cwd: cwd.clone(),
        context_files: snapshot.context_files.clone(),
        skills: snapshot.skills.clone(),
        docs: DocsPointers::default(),
        today: today(),
    };
    let system_prompt = SystemPromptBuilder::new().build(&prompt_inputs);

    // The dynamic-tool registry (Pi `_toolRegistry`): every Availability-visible tool, the caller's
    // custom tools, AND the extension-contributed/override tools are enable-able; the active set
    // starts at the build-time selection. Including the extension tools is load-bearing: (a) the
    // permission companion's registry / unknown-tool gate checks `all_tool_names` against this
    // registry (an extension tool absent here would be falsely blocked as "unknown"), and (b) a
    // `setActiveTools` rebuild (`DynamicToolState::set_active`) looks tools up BY NAME in this
    // registry — an extension override (recording/test double or a real replacement of a built-in)
    // must survive the rebuild rather than being replaced by the shadowed built-in. Extended LAST
    // so an override wins the `BTreeMap`-by-name dedup in `DynamicToolState::new`.
    let mut registry_tools = visible.clone();
    // The SDK-supplied custom tools go through the same registered-tool wrapper (Pi folds them
    // into `_baseToolDefinitions` and wraps that whole map, agent-session.ts:2507-2515), so a
    // custom tool that widens the active set also derives `addedToolNames`. `active_tools`
    // above already returned WRAPPED handles for the built-ins + extension tools.
    // Each SDK custom tool is also the SDK half of upstream's renderer map: `allCustomTools`
    // spreads `this._customTools` into the very map `getToolDefinition(name)` reads, so a
    // custom tool's own `renderCall`/`renderResult` reaches the transcript
    // (`core/agent-session.ts:2471-2495`, resolved at
    // `modes/interactive/components/tool-execution.ts:83-101` @v0.83.0). Registering here is
    // what gives `Tool::render_call`/`Tool::render_result` a reader at all — before this they
    // were overridable methods nothing in the workspace ever called, so a custom tool that
    // supplied its own rendering had it silently discarded and drew the generic shell.
    // Registered UNWRAPPED: the renderer belongs to the tool the caller configured, and
    // `wrap_tool`'s active-set diffing has nothing to add to a pure render call (the wrapper
    // delegates both methods through anyway, `cyrup-ext/src/wrapper.rs`).
    for tool in &cfg.custom_tools {
        ext_host.register_native_tool_renderer(tool.clone());
    }
    registry_tools.extend(cfg.custom_tools.iter().map(|t| ext_host.wrap_tool(t.clone())));
    registry_tools.extend(active_tools.iter().cloned());
    let contributions: std::collections::BTreeMap<String, ToolPromptContribution> = registry_tools
        .iter()
        .map(|t| (t.name().to_string(), tool_contribution(t)))
        .collect();
    // The rebuilder base = the prompt inputs with the per-run tool fields cleared (re-derived
    // from the active set on each `setActiveToolsByName`).
    let mut rebuild_base = prompt_inputs.clone();
    // CLEARED, not "explicitly zero tools": since SESS-016 `None` means "unset — use pi's
    // `selectedTools || [read,bash,edit,write]` default" and `Some(vec![])` means "the caller
    // genuinely restricted the agent to no tools", which suppresses the skills section and every
    // tool guideline. This placeholder is overwritten on every call
    // (`PromptRebuilder::rebuild` assigns `inputs.selected_tools = Some(active…)`, tools.rs:61),
    // so the value is never observed — but it must not READ as the restricted case.
    rebuild_base.selected_tools = None;
    rebuild_base.tool_contributions = Vec::new();
    // Shared with `host_services` so a loaded guest's `setActiveTools`/`getActiveTools`
    // capability read+mutates the SAME authoritative active-tool view the host/CLI toggle uses
    // (Pi binds both to `agent.state.tools`, agent-session.ts:2281,2283).
    let dynamic_tools = Arc::new(std::sync::Mutex::new(crate::tools::DynamicToolState::new(
        registry_tools,
        base_tools.clone(),
        crate::tools::PromptRebuilder::new(rebuild_base, contributions),
    )));
    host_services.attach_dynamic_tools(dynamic_tools.clone());
    // EXT-005: seed the guest-visible `ctx.getSystemPrompt()` / `ctx.isProjectTrusted()` reads
    // from the values this build resolved (Pi binds both straight to the session:
    // `getSystemPrompt: () => this.systemPrompt` and `isProjectTrusted: () =>
    // this.settingsManager.isProjectTrusted()`, agent-session.ts:2410,2434). Without this a
    // guest got the trait defaults — an empty prompt and a confident, wrong `false` for trust,
    // even in a project cyrup had just decided IS trusted.
    host_services.update_prompt_state(Some(system_prompt.clone()), settings.project_trusted());

    Ok(PromptSurface { context_store, active_tools, system_prompt, dynamic_tools })
}

/// `resolvePromptInput(source, description)`'s read leg (`resource-loader.ts:53-68` @v0.83.0), for a
/// source that `cyrup_resources::discover_system_prompt_file` already `exists()`-checked:
///
/// ```ts
/// if (existsSync(input)) {
///     try { return readFileSync(input, "utf-8"); }
///     catch (error) {
///         console.error(chalk.yellow(`Warning: Could not read ${description} file ${input}: ${error}`));
///         return input;
///     }
/// }
/// ```
///
/// The `return input` on a read failure is upstream's literal behaviour and is ported as such: the
/// PATH STRING becomes the prompt body. It looks wrong and it is faithful — `resolvePromptInput`
/// cannot distinguish "a path that failed to read" from "prompt text that happens to name a file",
/// so it falls back to the same branch as the not-a-path case. `cyrup/src/cli.rs`'s
/// `resolve_prompt_input` already ports the identical rung for the `--system-prompt` flag.
///
/// The warning goes to `tracing` rather than to `StartupDiagnostics`: upstream's is a bare
/// `console.error`, not a resource diagnostic, and `ResourceKind` has no variant for a prompt FILE
/// (its `Prompt` is the prompt-template family, which would file this under `[Prompt conflicts]`).
fn read_discovered_prompt(path: &Path, description: &str) -> Option<String> {
    match std::fs::read(path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) => {
            tracing::warn!(
                "Warning: Could not read {description} file {}: {e}",
                path.display()
            );
            Some(path.to_string_lossy().into_owned())
        }
    }
}

/// Today's date (UTC) for the prompt footer; falls back to the epoch on a clock fault.
fn today() -> time::Date {
    time::OffsetDateTime::now_utc().date()
}
