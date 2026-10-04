//! Runtime assembly: build the session factory, attach the native built-ins, launch the runtime,
//! and apply the post-build session knobs.
//!
//! **Why this module exists.** The native-extension attachment sequence used to be written out
//! three times — once in each of the interactive, RPC and print/json arms of `main.rs` — and the
//! three copies were byte-identical for all 45 statements. Every new extension had to be added in
//! three places, and the explanatory comments had already drifted apart between the copies. The
//! runtime-creation + diagnostics-gate + post-build sequence was likewise duplicated between the
//! RPC and print/json arms (22 identical statements).
//!
//! The arms differ in exactly three ways, and each is now a parameter rather than a fork in the
//! source:
//!
//! * the interactive host supplies a `trust_prompt` and the other two do not — pi's `hasUI` gate
//!   (project-trust.ts:86-88), SEAM-065;
//! * the modelless hard stop is mode-gated — pi `main.ts:852-855` runs it for
//!   `appMode !== "interactive"` only, so interactive launches modelless and shows a banner
//!   instead (SEAM-075). That is [`PostBuild::require_model`];
//! * the interactive arm follows the launch with `time("resolveModelScope")` +
//!   `bind_extensions()`, which stay at the call site where their pi citations sit.
//!
//! Two apparent differences are NOT parameterised, because they are provably equivalent:
//! `.context(…)?` on the `create_unannounced` result is exactly the
//! `Err(anyhow::Error::new(e).context(…))` form the non-interactive arms spelled out, and the
//! unconditional [`crate::output_guard::restore_stdout`] is a no-op under interactive —
//! [`crate::cli::should_take_over_stdout`] is `mode != AppMode::Interactive && …`, so interactive
//! never installs the guard and `restore_stdout` is a bare `AtomicBool` store.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use cyrup_config::{AuthStore, ConfigDirs, ModelFile, SettingsStore};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    AgentSession, AgentSessionRuntime, ScopedModel, SessionConfig, SessionFactory, SessionTarget,
    TrustPromptFn,
};

use crate::cli::Cli;
use crate::diagnostics::{self, Diagnostic};
use crate::timings;

/// Attach the native built-in extensions to `builder`, in pi's load order.
///
/// This is the ONE copy of a sequence that used to exist verbatim in all three mode arms. The
/// order is load-bearing:
///
/// 0. **llama.cpp** (EXT-027) goes FIRST, because pi loads its `builtInExtensions` ahead of
///    everything else: `extensionFactories = [...builtInExtensions, ...(options?.extensionFactories
///    ?? [])]` (`main.ts:569`) with `llama.cpp` the head of `builtInExtensions`
///    (`extensions/index.ts:7-8`). Attached unconditionally, in every mode and inside a subagent
///    child, exactly as upstream has it in every session: `llama_extension_for_env` never gates.
///    `--no-extensions` switches it off through `NativeExtension::is_ambient() -> true` (a child
///    is NOT among the natives its launcher re-injects, so it loses it too), and it stays out of
///    the startup `[Extensions]` list through `is_hidden` (`resource-loader.ts:729`,
///    `interactive-mode.ts:1778`) while remaining in `loaded_ids`.
/// 1. **Intercom** is BUILT first (`_concrete`) so its broker-backed delivery/clarify/steer seam
///    channels can be handed to the SubAgents extension via `with_channels` (the port doc §8.4
///    item 1 / P5 handoff — CLOSING R-SA-037/119/120/123/124/125). Child-mode gated: a subagent
///    child with orchestrator metadata always attaches so `contact_supervisor` registers; a plain
///    session attaches only when opted in (`_concrete` returns `None` otherwise, no broker).
/// 2. **SubAgents** is REGISTERED first, composing its opt-in gate with the T6 child-mode gate
///    (Pi `extension/index.ts:243-245` + `extension/fanout-child.ts:131`): a plain top-level
///    session attaches the orchestrator surface only when opted in (`is_installed`:
///    `CYRUP_SUBAGENTS` truthy, or a `subagents/config.json` at user/project scope); a plain child
///    registers nothing; a fanout-authorized child (`CYRUP_SUBAGENT_FANOUT_CHILD=1`) gets a
///    restricted, mutation-blocked tool REGARDLESS of `is_installed`. When intercom attached this
///    session its real channels are threaded in, else the NoTransport/NoOp degrade defaults stand
///    (R-SA-020).
/// 3. **The subagent prompt runtime** (SUBA-S01, pi `pi-args.ts:13`, which loads
///    `subagent-prompt-runtime.ts` into the child as its OWN extension): a plain subagent child
///    attaches NO subagents extension — `subagent_extension_for_env` returns `None` for it by
///    design — so the child-side `structured_output` tool cannot come from that gate. This one is
///    independent: it builds only when the parent passed both structured-output env vars, i.e.
///    only for a step that actually declared an `outputSchema`. Every other process attaches
///    nothing.
/// 4. **Intercom itself**, now that its channels have been handed out.
/// 5. **The MCP adapter** (gap-analysis 13a, MCP-001, the `pi-mcp-adapter` port). Attached
///    UNCONDITIONALLY — upstream is an installed npm package present in every session of every
///    mode, so there is no install gate to mirror; `--no-extensions` switches it off through
///    `NativeExtension::is_ambient() -> true`, which is the seam pi's own `noExtensions` uses on
///    the PATH tier that package lives in. It is NOT skipped inside a subagent child either: a
///    child resolves its `mcp:` tool selectors against the parent's `mcp-cache.json` and pins the
///    servers it needs with `MCP_DIRECT_TOOLS`. The `None` programmatic config is
///    `createMcpAdapter()`'s own default — discovery, not a caller-supplied config.
/// 6. **The permission system** (port doc §4): the opt-in allow/ask/deny gate over tool calls.
///    `permission_extension_for_env` selects the role by the `CYRUP_SUBAGENT_CHILD` signal — a
///    subagent child loads the gate with the child→parent ask-FORWARDING channel (P-4,
///    forwarding.rs), a root session loads it with the in-session dialog + the forwarding watcher
///    — and returns `None` when the gate is not installed (DI-5) OR when an installed gate is
///    switched off by `"enabled": false` in its `config.json` (v0.8.0 `index.ts:1473-1477`, the
///    master switch that early-returns before registration).
/// 7. **Flux** (spec/flux.md §3.4.5): the pipeline's bundled prompt templates + the three native
///    renderers. Attached unconditionally at the top level — unlike the three above there is no
///    install gate, because the whole point of moving flux into the binary is that it works with
///    no install step — the templates are EMBEDDED and materialised under `<agent_dir>/flux/`
///    on `ResourcesDiscover` (FLUX-001), which is why it takes `agent_dir` like the two above.
///    `flux_extension_for_env` still returns `None` inside a subagent CHILD: a
///    child re-execs this binary in Print/Json mode, and contributing 15 templates plus a skill to
///    every child would put the skill into every child's system prompt for a pipeline the child is
///    not running.
fn attach_native_extensions(
    mut builder: SessionFactory,
    dirs: &ConfigDirs,
    session_cwd: PathBuf,
) -> anyhow::Result<SessionFactory> {
    let agent_dir: &Path = &dirs.agent_dir;
    if let Some(ext) = cyrup_llama::llama_extension_for_env(agent_dir) {
        builder = builder.with_native_extension(ext);
    }
    let intercom_ext = cyrup_intercom::intercom_extension_for_env_concrete(
        agent_dir.to_path_buf(),
        session_cwd.clone(),
    )
    .map_err(|e| anyhow::anyhow!("building intercom extension: {e}"))?;
    let subagent_ext = match &intercom_ext {
        Some(ic) => cyrup_ext_subagents::extension::subagent_extension_for_env_with_channels(
            agent_dir,
            crate::subagent_config::load_subagent_extension_config(dirs),
            session_cwd.clone(),
            ic.delivery_channel(),
            ic.clarify_channel(),
            ic.steer_channel(),
        ),
        None => cyrup_ext_subagents::extension::subagent_extension_for_env(
            agent_dir,
            crate::subagent_config::load_subagent_extension_config(dirs),
            session_cwd.clone(),
        ),
    };
    if let Some(ext) = subagent_ext {
        builder = builder.with_native_extension(ext);
    }
    // CFG-080: the prompt runtime REFUSES to build when the tool budget the parent shipped in
    // `CYRUP_SUBAGENT_TOOL_BUDGET` does not decode — pi's `decodeToolBudgetEnv` throws out of
    // `registerSubagentPromptRuntime` (`pi-subagents` v0.64.0
    // `src/runs/shared/subagent-prompt-runtime.ts:693`, `tool-budget.ts:74-80`) and its loader
    // discards the extension. Carrying the error out of the launch path is how this process
    // declines to run a child whose budget was silently removed; the message is pi's own.
    let prompt_runtime = cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_for_env()
        .map_err(|e| anyhow::anyhow!("building the subagent prompt runtime: {e}"))?;
    if let Some(runtime) = prompt_runtime {
        builder = builder.with_native_extension(runtime);
    }
    if let Some(ic) = intercom_ext {
        builder = builder.with_native_extension(ic);
    }
    if let Some(ext) = cyrup_mcp::mcp_extension_for_env(agent_dir, None, session_cwd.clone()) {
        builder = builder.with_native_extension(ext);
    }
    if let Some(ext) =
        cyrup_permission_system::permission_extension_for_env(agent_dir.to_path_buf(), session_cwd)
    {
        builder = builder.with_native_extension(ext);
    }
    if let Some(ext) = cyrup_flux::flux_extension_for_env(agent_dir) {
        builder = builder.with_native_extension(ext);
    }
    Ok(builder)
}

/// Build the [`SessionFactory`] every mode launches from: the shared prefix plus
/// [`attach_native_extensions`].
///
/// `trust_prompt` is `Some` for the interactive host ONLY — SEAM-065. pi supplies its
/// `resolveProjectTrust` prompt callback only where it has a UI (`hasUI`, project-trust.ts:86-88);
/// every other host leaves it unset and the builder falls through to untrusted, exactly as pi's
/// `if (!hasUI) return false;` does. The trust STORE is not mode-gated and is wired for all three:
/// pi's `resolveProjectTrusted` reads it at project-trust.ts:72-75 for every host.
pub fn build_factory(
    provider: Arc<dyn Provider>,
    config: SessionConfig,
    settings_store: Arc<dyn SettingsStore>,
    auth_store: Arc<AuthStore>,
    dirs: &ConfigDirs,
    models_json: Arc<ModelFile>,
    trust_prompt: Option<TrustPromptFn>,
) -> anyhow::Result<Arc<SessionFactory>> {
    let session_cwd = config.cwd.clone();
    // The resolver's providers read the SAME store `/login` writes through (`.auth` below).
    let credentials = cyrup_config::login::runtime_credentials(auth_store.clone());
    let mut builder = SessionFactory::new(provider, config)
        .settings_store(settings_store)
        .auth(auth_store)
        .trust_store(crate::prelaunch::trust_store_for(dirs));
    if let Some(prompt) = trust_prompt {
        builder = builder.trust_prompt(prompt);
    }
    builder = builder.provider_resolver(Arc::new(crate::provider::BuiltinProviderResolver::new(
        models_json,
        credentials,
    )));
    Ok(Arc::new(attach_native_extensions(
        builder,
        dirs,
        session_cwd,
    )?))
}

/// The per-run, post-build session knobs, and whether pi's modelless hard stop applies to this
/// host. See [`apply_post_build`] for the knobs themselves.
pub struct PostBuild<'a> {
    /// The trimmed `--name` display name (pi `appendSessionInfo`, main.ts:586).
    pub session_name: Option<&'a str>,
    pub cli: &'a Cli,
    /// Whether this is a brand-new session (pi `!hasExistingSession`, main.ts:394).
    pub fresh: bool,
    /// pi `main.ts:852-855` — `if (appMode !== "interactive" && !session.model) { … exit(1) }`.
    /// `false` for the interactive host, which launches modelless and shows the
    /// `modelFallbackMessage` as a banner instead (SEAM-075).
    pub require_model: bool,
    /// The startup settings manager's diagnostics, merged with the runtime's at the checkpoint
    /// (pi `startupSettingsDiagnostics`, main.ts:657, :896). CFG-088.
    pub startup_diagnostics: &'a [crate::diagnostics::Diagnostic],
    /// Whether this is the interactive host: its merged diagnostics are handed to the TUI rather
    /// than printed (pi `appMode !== "interactive"`, main.ts:898).
    pub interactive: bool,
}

/// A runtime that passed [`launch`]'s checkpoints.
pub struct Launched {
    pub runtime: Arc<AgentSessionRuntime>,
    pub session: Arc<AgentSession>,
    /// The merged startup diagnostics an interactive run shows in its transcript (pi
    /// `InteractiveMode({ startupDiagnostics })`, main.ts:936); always empty for the other hosts,
    /// which printed them. CFG-088.
    pub notices: Vec<crate::diagnostics::Diagnostic>,
}

/// Create the runtime, run pi's post-creation diagnostics checkpoint, apply the post-build knobs,
/// and apply the mode-gated modelless stop.
///
/// [`ControlFlow::Break`] carries the exit code the caller must return; [`ControlFlow::Continue`]
/// carries the live runtime + session (and an interactive run's startup notices).
///
/// SEAM-033 — `create_unannounced` is pi's `createAgentSessionRuntime`
/// (agent-session-runtime.ts:414-432), which never emits `session_start`; the HOST announces.
/// `--name` (pi main.ts:650) and the scoped `--models` (pi main.ts:742-750) are applied in the
/// window this opens, so an extension's `session_start` handler observes the configured session
/// rather than an unnamed one on the pre-scope model. For print/json this also binds the
/// self-handle (via `into_shared`) so the post-run loop — auto-retry, post-run auto-compaction,
/// queued continuations — fires for one-shot runs.
///
/// AGENT-027 — pi's `main` timing sequence is ONE linear path covering every mode, so the three
/// runtime marks (main.ts:792/:798/:850) are taken here for all three arms.
pub async fn launch(
    factory: Arc<SessionFactory>,
    target: SessionTarget,
    post: PostBuild<'_>,
) -> anyhow::Result<ControlFlow<i32, Launched>> {
    timings::time("createRuntime", timings::TimingLabel::Main);
    let runtime = AgentSessionRuntime::create_unannounced(factory, target)
        .await
        .context("building agent session runtime")?;
    timings::time("createAgentSessionRuntime", timings::TimingLabel::Main);

    // `--help` (CFG-088 / SEAM-020) — pi's one help exit, `main.ts:857-864` @v0.87.1:
    //
    // ```ts
    // if (parsed.help) {
    //     reportDiagnostics(startupSettingsDiagnostics);
    //     const extensionFlags = resourceLoader.getExtensions()
    //         .extensions.flatMap((extension) => Array.from(extension.flags.values()));
    //     printHelp(extensionFlags);
    //     process.exit(0);
    // }
    // ```
    //
    // It sits HERE, straight after the runtime is built and BEFORE the diagnostics checkpoint
    // below (`:896-904`), because both halves of its output need the runtime and neither may wait
    // for the checkpoint: the flag list is the loaded extensions' declarations, and the startup
    // settings diagnostics are printed on their own — a runtime error (a conflicting extension
    // flag, say) does not stop `--help`. `process.exit(0)` runs no teardown, so neither does this:
    // `create_unannounced` never announced the session, and nothing is shut down.
    if post.cli.help {
        diagnostics::report(post.startup_diagnostics);
        let session = runtime.session().await;
        // `console.log` — through the stdout guard, which a `--mode json --help` run has
        // installed (pi's takeover is not lifted for `--help` either, `main.ts:639-642`).
        crate::output_guard::emit_stray(&crate::cli::render_help(
            &session.extension_flag_declarations(),
        ));
        return Ok(ControlFlow::Break(0));
    }

    // `--list-models [search]` (SEAM-135) — pi's second runtime-metadata exit, the sibling of the
    // `--help` one above and in the same place, `main.ts:866-871` @v0.87.1:
    //
    // ```ts
    // if (parsed.listModels !== undefined) {
    //     reportDiagnostics(startupSettingsDiagnostics);
    //     const searchPattern = typeof parsed.listModels === "string" ? parsed.listModels : undefined;
    //     await listModels(modelRuntime, searchPattern, AbortSignal.timeout(15_000));
    //     process.exit(0);
    // }
    // ```
    //
    // Running it on the built session (not before runtime creation, where cyrup used to exit) is
    // what lets the listing see a model an extension registered with `registerProvider`. Same
    // teardown story as `--help`: `process.exit(0)` runs none, and `create_unannounced` announced
    // nothing.
    if let Some(search) = post.cli.list_models.as_deref() {
        diagnostics::report(post.startup_diagnostics);
        let session = runtime.session().await;
        return Ok(ControlFlow::Break(crate::actions::list_models_for_session(
            &session, search,
        )));
    }

    // Pi main.ts:895-904 (SEAM-S01, CFG-088) — report the merged startup + runtime diagnostics
    // and exit 1 on any runtime error. Same checkpoint, every mode.
    let report =
        diagnostics::report_runtime(&runtime, post.startup_diagnostics, post.interactive).await;
    if report.fatal {
        runtime.dispose().await;
        crate::output_guard::restore_stdout();
        return Ok(ControlFlow::Break(1));
    }
    // pi's `time("createAgentSession")` sits directly after the same diagnostics gate
    // (main.ts:843-850).
    timings::time("createAgentSession", timings::TimingLabel::Main);
    let session = runtime.session().await;
    apply_post_build(&session, post.session_name, post.cli, post.fresh).await;

    // Pi main.ts:852-855 — the modelless hard stop, gated on the MODE:
    //   `if (appMode !== "interactive" && !session.model) {`
    //   `    console.error(chalk.red(formatNoModelsAvailableMessage()));`
    //   `    process.exit(1);`
    //   `}`
    // It lives HERE, on the built session, not in the builder: `sdk.ts:216-218` resolves a
    // credential-less start to `model: undefined` + a `modelFallbackMessage` banner rather than an
    // error (SEAM-075), so every mode reaches this point and only the non-interactive ones stop.
    // Placed after `apply_post_build` because pi applies `--name` (main.ts:650) and the scoped
    // `--models` (main.ts:742-750) before :852.
    if post.require_model && session.model().is_none() {
        runtime.dispose().await;
        crate::output_guard::restore_stdout();
        diagnostics::no_models_available();
        return Ok(ControlFlow::Break(1));
    }
    Ok(ControlFlow::Continue(Launched {
        runtime,
        session,
        notices: report.notices,
    }))
}

/// Apply the per-run, post-build session knobs that have no `SessionConfig` slot: the trimmed
/// `--name` display name (Pi `appendSessionInfo`, main.ts:586) and the `--models` Ctrl+P scope (Pi
/// `resolveModelScope`/`scopedModels`, main.ts:685).
///
/// The scope patterns follow Pi's precedence `parsed.models ?? settingsManager.getEnabledModels()`
/// (main.ts:685): an explicit `--models` wins, otherwise the persisted `enabledModels` setting is the
/// fallback scope source. Matching itself is delegated to `cyrup-config`'s `minimatch`-faithful
/// resolver (see [`resolve_scoped_models_reporting`]), not a bespoke matcher.
///
/// `fresh` is whether this is a brand-new session (Pi `!hasExistingSession`, main.ts:394): a resumed
/// session keeps its own restored model, so the saved-default-in-scope active-model pick only fires
/// for a fresh session.
async fn apply_post_build(session: &AgentSession, name: Option<&str>, cli: &Cli, fresh: bool) {
    if let Some(name) = name {
        let _ = session.set_session_name(name).await;
    }
    // Pi `modelPatterns = parsed.models ?? settingsManager.getEnabledModels()` (main.ts:685): an
    // explicit `--models` wins; otherwise fall back to the persisted `enabledModels` setting.
    let patterns: Vec<String> = if cli.models.is_empty() {
        session
            .services()
            .settings
            .effective()
            .enabled_models()
            .unwrap_or_default()
    } else {
        cli.models.clone()
    };
    if !patterns.is_empty() {
        let catalog = session.model_catalog();
        // Pi `resolveModelScope` prints EVERY diagnostic its `WithDiagnostics` sibling collected —
        // `console.warn(chalk.yellow(`Warning: ${diagnostic.message}`))`, model-resolver.ts:355-361 —
        // before returning the (possibly empty) scope, and does so on the live path at main.ts:741-743
        // for both `--models` and the `enabledModels` fallback. Without this a typo'd
        // `--models "anthropc/*"` scoped nothing with no output at all.
        let (scoped, diags) = resolve_scoped_models_reporting(&catalog, &patterns);
        diagnostics::report(&diags);
        if !scoped.is_empty() {
            // The saved-default-in-scope active-model pick (Pi `buildSessionOptions`, main.ts:394-414):
            // when `--models` scopes the set and `--model` is omitted, the active model is the saved
            // default if it is in scope, else the first scoped model. Apply only on a fresh session.
            if cli.model.is_none() && fresh {
                let eff = session.services().settings.effective();
                if let Some(chosen) = pick_scoped_active_model(
                    &scoped,
                    eff.default_provider().as_deref(),
                    eff.default_model().as_deref(),
                ) {
                    let model = chosen.model.clone();
                    let thinking = chosen.thinking_level;
                    if session.set_model_resolved(model).await.is_ok() {
                        // Use the scoped model's thinking level only when `--thinking` was omitted
                        // (explicit `--thinking` takes precedence and is applied by the builder).
                        if cli.thinking.is_none()
                            && let Some(level) = thinking
                        {
                            let _ = session.set_thinking_level(level).await;
                        }
                    }
                }
            }
            session.set_scoped_models(scoped);
        }
    }
}

/// The saved-default-in-scope active-model pick (Pi `buildSessionOptions`, main.ts:394-414): given the
/// resolved `--models` scope and the settings default `(provider, model)`, prefer the saved default
/// when it is a member of the scope (case-insensitive `provider`+`id` match, Pi `modelsAreEqual`),
/// else the first scoped model. `None` only when the scope is empty.
fn pick_scoped_active_model<'a>(
    scoped: &'a [ScopedModel],
    saved_provider: Option<&str>,
    saved_model: Option<&str>,
) -> Option<&'a ScopedModel> {
    let saved = match (saved_provider, saved_model) {
        (Some(provider), Some(model)) if !provider.is_empty() && !model.is_empty() => {
            scoped.iter().find(|sm| {
                sm.model.provider.as_str().eq_ignore_ascii_case(provider)
                    && sm.model.id.as_str().eq_ignore_ascii_case(model)
            })
        }
        _ => None,
    };
    saved.or_else(|| scoped.first())
}

/// Pi `resolveModelScope(available, patterns)` in ONE call — both halves of its
/// `{ scopedModels, diagnostics }` return (model-resolver.ts:269-350 @v0.83.0).
///
/// CFG-008's residual. The diagnostics used to be REPLAYED in this binary: a per-pattern loop that
/// re-ran `resolve_scope` on a one-element slice to test emptiness, plus a hand-rolled recursion
/// that re-derived `parseModelPattern`'s colon-stripping to recover pi's `Invalid thinking level
/// "X" in pattern "Y". Using default instead.` sentence. Both of those now come from the resolver
/// that already computes them (`ModelScopeDiagnostic`, model.rs), so the glob/non-glob split, the
/// `:level` stripping and the exact-reference short-circuit exist in exactly one place.
///
/// Two notes the deleted replay carried, now settled rather than dropped:
///
/// * its `[CYRUP-DELTA]` about the missing `findExactModelReferenceMatch` short-circuit (:297-303)
///   was STALE — CFG-018 put the short-circuit into `resolve_scope_reporting`'s glob arm, so a
///   literal id containing `[` or `?` resolves instead of warning;
/// * its claim that `cyrup-config` "abbreviates the text to `invalid thinking level '<suffix>'`"
///   was also stale — `parse_pattern` mints pi's full sentence (model.rs, Pi `:243`).
///
/// The matching itself stays where it was: `cyrup-config`'s byte-for-byte
/// `minimatch({ nocase: true })` port (13,877-case verified) — a pattern containing `*`/`?`/`[` is
/// matched with real path-segment-aware globbing (`*` never crosses `/`, full `?`/`[...]`/`{a,b}`/
/// extglob support), and a non-glob pattern resolves to Pi's single best (alias-preferred,
/// `localeCompare`-tie-broken) model. This replaced the bespoke `*`-only, non-path-segment-aware
/// substring matcher that once lived in `cli.rs` (e.g. `anthropic*` wrongly matched every anthropic
/// model; `[...]` classes were unsupported). The resolver's `ScopedModel` is mapped onto the
/// session-svc `ScopedModel` here.
fn resolve_scoped_models_reporting(
    catalog: &[cyrup_provider::Model],
    patterns: &[String],
) -> (Vec<ScopedModel>, Vec<Diagnostic>) {
    let result = cyrup_config::ModelResolver::new(catalog).resolve_scope_reporting(patterns);
    let scoped = result
        .models
        .into_iter()
        .map(|sm| ScopedModel {
            model: sm.model,
            thinking_level: sm.thinking_level,
        })
        .collect();
    // Pi's rendering loop: `for (const diagnostic of diagnostics) console.warn(chalk.yellow(
    // `Warning: ${diagnostic.message}`))` (model-resolver.ts:355-361), reached on the live path at
    // main.ts:741-743 for both `--models` and the `enabledModels` fallback.
    let diagnostics = result
        .diagnostics
        .into_iter()
        .map(|d| Diagnostic::warning(d.message))
        .collect();
    (scoped, diagnostics)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use crate::diagnostics::DiagnosticLevel;

    use super::{ScopedModel, pick_scoped_active_model, resolve_scoped_models_reporting};

    /// The models half of the one scope pass, for the tests that only assert matching.
    fn resolve_scoped_models_reporting_models(
        catalog: &[cyrup_provider::Model],
        patterns: &[String],
    ) -> Vec<ScopedModel> {
        resolve_scoped_models_reporting(catalog, patterns).0
    }

    /// The diagnostics half of the one scope pass.
    fn resolve_scoped_models_reporting_diagnostics(
        catalog: &[cyrup_provider::Model],
        patterns: &[String],
    ) -> Vec<super::Diagnostic> {
        resolve_scoped_models_reporting(catalog, patterns).1
    }

    /// The `--models`/`enabledModels` scope must report Pi's diagnostics, not resolve in silence
    /// (Pi `resolveModelScopeWithDiagnostics` → `resolveModelScope`, model-resolver.ts:270-361;
    /// live path main.ts:741-743). Before the fix `resolve_scope` returned only the matched set and
    /// `apply_post_build` dropped everything else on the floor, so a typo'd pattern was a silent
    /// no-op.
    #[test]
    fn scope_diagnostics_report_no_match_and_invalid_thinking_level_like_pi() {
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());

        // A pattern that matches nothing warns, in BOTH arms — the glob arm
        // (model-resolver.ts:311-318) and the non-glob arm (:334-341).
        for pattern in ["anthropc/*", "no-such-model-anywhere"] {
            let diags =
                resolve_scoped_models_reporting_diagnostics(&catalog, &[pattern.to_string()]);
            assert_eq!(diags.len(), 1, "{pattern}: {diags:?}");
            let only = diags.first().expect("one diagnostic");
            assert_eq!(only.level, DiagnosticLevel::Warning);
            assert_eq!(
                only.message,
                format!("No models match pattern \"{pattern}\"")
            );
        }

        // A pattern that DOES match is silent.
        assert!(
            resolve_scoped_models_reporting_diagnostics(&catalog, &["anthropic/*".to_string()])
                .is_empty(),
            "a matching pattern emits no diagnostic"
        );

        // An invalid `:level` suffix on a resolving pattern warns with Pi's exact sentence
        // (`parseModelPattern`, model-resolver.ts:243) and does NOT also warn no-match — the model
        // still resolves, at the default thinking level.
        let diags = resolve_scoped_models_reporting_diagnostics(
            &catalog,
            &["claude-opus-4-8:hihg".to_string()],
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags.first().expect("one diagnostic").message,
            "Invalid thinking level \"hihg\" in pattern \"claude-opus-4-8:hihg\". Using default instead."
        );

        // A VALID `:level` is not a diagnostic at all.
        assert!(
            resolve_scoped_models_reporting_diagnostics(
                &catalog,
                &["claude-opus-4-8:high".to_string()]
            )
            .is_empty(),
            "a valid thinking level is silent"
        );

        // Both warnings can ride on one pattern list, in pattern order.
        let diags = resolve_scoped_models_reporting_diagnostics(
            &catalog,
            &["claude-opus-4-8:hihg".to_string(), "anthropc/*".to_string()],
        );
        assert_eq!(diags.len(), 2, "{diags:?}");
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert!(
            messages
                .first()
                .is_some_and(|m| m.starts_with("Invalid thinking level"))
        );
        assert!(
            messages
                .get(1)
                .is_some_and(|m| m.starts_with("No models match pattern"))
        );
    }

    /// CFG-008's residual. Pi's `resolveModelScope` returns `{ scopedModels, diagnostics }` from ONE
    /// pass (model-resolver.ts:269-350 @v0.83.0) and the live call site consumes both
    /// (main.ts:741-743). cyrup resolved twice — once for the models, once more per pattern to
    /// re-derive the warnings — so the diagnostic TYPE lived in this binary as a replay.
    ///
    /// This pins the pairing: for a list mixing a hit, a miss and a bad thinking level, ONE call
    /// yields the scoped set AND both warnings, in pattern order, and the two legacy entry points
    /// are the two halves of exactly that result.
    #[test]
    fn one_scope_pass_returns_pis_models_and_diagnostics_together() {
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());
        let patterns = vec![
            "anthropic/*".to_string(),
            "anthropc/*".to_string(),
            "claude-opus-4-8:hihg".to_string(),
        ];

        let (scoped, diagnostics) = resolve_scoped_models_reporting(&catalog, &patterns);

        // Presence before absence: the good pattern really did scope something.
        assert!(!scoped.is_empty(), "`anthropic/*` scopes a non-empty set");
        assert!(
            scoped
                .iter()
                .any(|s| s.model.id.as_str() == "claude-opus-4-8"),
            "the `:hihg` pattern still resolves its prefix — Pi keeps the model and drops the level"
        );
        assert!(
            scoped
                .iter()
                .filter(|s| s.model.id.as_str() == "claude-opus-4-8")
                .all(|s| s.thinking_level.is_none()),
            "an invalid level yields `thinkingLevel: undefined` (model-resolver.ts:237-245)"
        );

        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            vec![
                "No models match pattern \"anthropc/*\"",
                "Invalid thinking level \"hihg\" in pattern \"claude-opus-4-8:hihg\". Using default \
                 instead.",
            ],
            "diagnostics come back in pattern order, from the resolver"
        );
        assert!(
            diagnostics
                .iter()
                .all(|d| matches!(d.level, DiagnosticLevel::Warning)),
            "Pi renders every scope diagnostic through `console.warn` (model-resolver.ts:355-361)"
        );
    }

    /// The live `--models`/`enabledModels` scope resolution must go through `cyrup-config`'s
    /// `minimatch`-faithful `ModelResolver::resolve_scope`, NOT the removed bespoke `*`-only matcher
    /// (gap-analysis 06 Gap #1; Pi `resolveModelScope`, model-resolver.ts:269-339). These are the
    /// exact divergences the crude matcher got wrong, verified against a real bundled catalog.
    #[test]
    fn resolve_scoped_models_uses_minimatch_semantics_like_pi() {
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());

        // Path-segment awareness: a 1-segment pattern (`anthropic*`, no `**`) can NEVER match the
        // 2-segment `anthropic/<id>` form under minimatch. The old crude matcher wrongly matched
        // EVERY anthropic model (its single segment anchored via plain substring across the `/`).
        //
        // It CAN match a bare id that genuinely begins "anthropic", and since `amazon-bedrock` was
        // ported that is no longer a hypothetical: Bedrock ids are dotted, e.g.
        // `anthropic.claude-opus-4-7`. So the assertion is no longer "zero matches" — it is that
        // every match is a BARE dotted id and none is the provider-qualified `anthropic/…` form.
        // Asserting emptiness here would have quietly re-encoded "amazon-bedrock is not ported".
        let scoped = resolve_scoped_models_reporting_models(&catalog, &["anthropic*".to_string()]);
        assert!(
            scoped.iter().all(|m| !m.model.id.as_str().contains('/')),
            "`anthropic*` is one segment, so it must never match the 2-segment `anthropic/<id>` \
             form; got {:?}",
            scoped
                .iter()
                .map(|m| m.model.id.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            scoped
                .iter()
                .all(|m| m.model.id.as_str().starts_with("anthropic")),
            "every match must actually begin with the literal pattern prefix; got {:?}",
            scoped
                .iter()
                .map(|m| m.model.id.as_str())
                .collect::<Vec<_>>()
        );

        // Character classes (`[68]`) are real minimatch syntax the crude matcher could not express
        // (it fell through to a literal-substring miss). Pi matches exactly the -6 and -8 opus ids.
        // (This used to read `[08]`; `claude-opus-4-0` was retired upstream in pi `cc2db980` — see
        // cyrup-provider `tests/catalog_data.rs`, PROV-004.)
        let scoped = resolve_scoped_models_reporting_models(
            &catalog,
            &["anthropic/claude-opus-4-[68]".to_string()],
        );
        let ids: Vec<&str> = scoped.iter().map(|s| s.model.id.as_str()).collect();
        assert!(
            ids.contains(&"claude-opus-4-6") && ids.contains(&"claude-opus-4-8"),
            "`anthropic/claude-opus-4-[68]` char-class must scope both opus ids, got {ids:?}"
        );
        assert!(
            scoped
                .iter()
                .all(|s| s.model.provider.as_str() == "anthropic"),
            "char-class stays path-segment-scoped to the anthropic provider"
        );

        // A bare `provider/*` glob is path-segment-aware: its first segment matches the whole
        // provider segment. Pi matches `minimatch(fullId) || minimatch(id)`, so every scoped model
        // is either an anthropic-provider model (fullId `anthropic/<id>`) or a model whose bare id
        // itself begins `anthropic/` (e.g. openrouter's `anthropic/claude-…`) — never an unrelated
        // provider like `anthropicX/…` (segment boundary, not a substring).
        let scoped = resolve_scoped_models_reporting_models(&catalog, &["anthropic/*".to_string()]);
        assert!(!scoped.is_empty(), "`anthropic/*` scopes a non-empty set");
        assert!(
            scoped
                .iter()
                .any(|s| s.model.provider.as_str() == "anthropic"),
            "`anthropic/*` includes the anthropic provider's own models"
        );
        assert!(
            scoped
                .iter()
                .all(|s| s.model.provider.as_str() == "anthropic"
                    || s.model.id.as_str().starts_with("anthropic/")),
            "every `anthropic/*` match is anthropic-provider or an `anthropic/`-prefixed id (Pi's \
             `minimatch(fullId) || minimatch(id)`)"
        );
    }

    fn scoped(provider: &str, id: &str) -> ScopedModel {
        // Build a `ScopedModel` from a real catalog entry so the pick exercises real `Model` fields.
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());
        let model = catalog
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == id)
            .or_else(|| catalog.iter().find(|m| m.provider.as_str() == provider))
            .expect("a catalog model for the provider")
            .clone();
        ScopedModel {
            model,
            thinking_level: None,
        }
    }

    // ============================================================================================
    // EXT-027 — the llama.cpp built-in is attached by `attach_native_extensions`.
    //
    // Every test drives the production seam, `build_factory` (the one caller of
    // `attach_native_extensions`), then builds a REAL session off the factory and reads what the
    // extension host actually loaded. Nothing here constructs the extension by hand, so removing the
    // `with_native_extension` line in `attach_native_extensions` fails every one of them.
    // ============================================================================================

    use std::sync::Arc;

    use cyrup_provider::Provider;
    use cyrup_provider::faux::FauxProvider;
    use cyrup_session_svc::{AgentSession, SessionConfig, TrustPromptFn};

    use super::build_factory;

    /// The id both [`cyrup_llama::LlamaExtension`] and the provider it registers carry
    /// (`provider.ts` `LLAMA_PROVIDER_ID`).
    const LLAMA_ID: &str = cyrup_llama::LLAMA_PROVIDER_ID;

    /// The always-attached sibling that follows llama.cpp in pi's order (`extensions/index.ts:7-12`
    /// lists llama.cpp before `mcp`); cyrup's MCP adapter is `ambient` like it.
    const MCP_ID: &str = cyrup_mcp::EXTENSION_ID;

    /// Environment marker: when set, [`llama_child_probe`] is the BODY of a probe run inside a
    /// re-executed test binary rather than a no-op. Its value is `keep` or `no-extensions`.
    const PROBE_ENV: &str = "CYRUP_LLAMA_ATTACH_PROBE";

    /// The subagent-child marker `SessionBuilder` reads (`cyrup_ext_subagents::spawn::nested_events::CHILD_ENV`).
    const CHILD_ENV: &str = "CYRUP_SUBAGENT_CHILD";

    /// A trust prompt that is never consulted (`trust_override` answers first) — it exists so the
    /// interactive arm's `Some(prompt)` shape is the one under test (pi's `hasUI` gate,
    /// project-trust.ts:86-88).
    fn never_asked_trust_prompt() -> TrustPromptFn {
        Arc::new(|_, _| Box::pin(async { Some(true) }))
    }

    /// What a session built through `build_factory` reports: every id the host loaded (in load
    /// order) and the ids the startup `[Extensions]` panel lists.
    struct Loaded {
        loaded: Vec<String>,
        listed: Vec<String>,
    }

    /// Build a session exactly as a mode arm does — through `build_factory` — over the faux
    /// provider in a hermetic temp home. `interactive` selects the one thing the three arms differ
    /// by in this function: whether a `trust_prompt` is supplied.
    async fn session_through_build_factory(interactive: bool, no_extensions: bool) -> Loaded {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let env = cyrup_config::EnvVars {
            home: Some(agent_dir.clone()),
            ..cyrup_config::EnvVars::default()
        };
        let overrides = cyrup_config::CliConfigOverrides {
            agent_dir: Some(agent_dir.clone()),
            cwd: Some(cwd.clone()),
            ..Default::default()
        };
        let dirs = cyrup_config::ConfigDirs::resolve(&overrides, &env).unwrap();

        let mut config = SessionConfig::new(cwd, agent_dir.clone());
        config.persist = false;
        config.trust_override = Some(true);
        config.no_extensions = no_extensions;
        let target = config.target.clone();

        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let factory = build_factory(
            provider,
            config,
            crate::file_settings_store(&dirs),
            Arc::new(cyrup_config::AuthStore::at(agent_dir.join("auth.json"))),
            &dirs,
            Arc::new(cyrup_config::ModelFile::default()),
            interactive.then(never_asked_trust_prompt),
        )
        .unwrap();
        let session: AgentSession = factory.build(target, None).await.unwrap();

        let loaded = session
            .services()
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        let listed = cyrup_tui::StartupReport::from_session(&session, false).extensions;
        Loaded { loaded, listed }
    }

    /// The position of `id` in `ids`, failing loudly (with the whole list) when absent.
    fn position(ids: &[String], id: &str) -> usize {
        ids.iter()
            .position(|i| i == id)
            .unwrap_or_else(|| panic!("{id} is not loaded; got {ids:?}"))
    }

    /// Attached in all three modes `attach_native_extensions` serves — interactive (a trust prompt
    /// is supplied) and RPC / print / json (it is not) — and FIRST in the order, ahead of the
    /// always-attached MCP adapter (pi `main.ts:569`: `[...builtInExtensions, ...user factories]`,
    /// `extensions/index.ts:7-12`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn llama_is_attached_first_in_every_mode() {
        for (mode, interactive) in [("interactive", true), ("rpc/print/json", false)] {
            let Loaded { loaded, .. } = session_through_build_factory(interactive, false).await;
            assert_eq!(
                loaded.first().map(String::as_str),
                Some(LLAMA_ID),
                "{mode}: llama.cpp is loaded before every other native (pi main.ts:569); got {loaded:?}"
            );
            assert!(
                position(&loaded, LLAMA_ID) < position(&loaded, MCP_ID),
                "{mode}: llama.cpp precedes the MCP adapter (extensions/index.ts:7-12); got {loaded:?}"
            );
        }
    }

    /// Hidden at startup, loaded regardless: pi lists `extensions.filter((e) => !e.hidden)`
    /// (`interactive-mode.ts:1778`) and marks every `builtin:` extension hidden
    /// (`resource-loader.ts:729`). The MCP adapter in the same breath proves the list is not simply
    /// empty.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn llama_is_loaded_but_absent_from_the_startup_extensions_list() {
        let Loaded { loaded, listed } = session_through_build_factory(true, false).await;
        assert!(
            loaded.iter().any(|i| i == LLAMA_ID),
            "llama.cpp is in loaded_ids; got {loaded:?}"
        );
        assert!(
            !listed.iter().any(|i| i == LLAMA_ID),
            "llama.cpp is hidden from the startup [Extensions] list; got {listed:?}"
        );
        assert!(
            listed.iter().any(|i| i == MCP_ID),
            "a non-hidden native IS listed, so the list is not trivially empty; got {listed:?}"
        );
    }

    /// `--no-extensions` drops it: pi's `builtin:llama.cpp` is a path in the tier the flag collapses
    /// (`package-manager.ts:972-974`, `resource-loader.ts:569-571`), which cyrup spells
    /// `is_ambient() -> true` (builder.rs:2647-2657). The same flag-off session keeps it, so this
    /// is the flag's effect and not a session that never had it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_extensions_drops_llama_in_every_mode() {
        for (mode, interactive) in [("interactive", true), ("rpc/print/json", false)] {
            let kept = session_through_build_factory(interactive, false)
                .await
                .loaded;
            assert!(kept.iter().any(|i| i == LLAMA_ID), "{mode}: {kept:?}");
            let dropped = session_through_build_factory(interactive, true)
                .await
                .loaded;
            assert!(
                !dropped.iter().any(|i| i == LLAMA_ID),
                "{mode}: --no-extensions drops the ambient llama.cpp built-in; got {dropped:?}"
            );
        }
    }

    /// A subagent CHILD, `CYRUP_SUBAGENT_CHILD` set, treats llama.cpp like the other ambient natives
    /// (builder.rs:2629-2633): attached without the flag, dropped under `--no-extensions` — it is
    /// not one of the three runtime natives a child's launcher re-injects, so the child carve-out
    /// does not rescue it. The MCP adapter (ambient too) is asserted in the same states to pin
    /// "like the other ambient natives".
    ///
    /// The environment is process-global and `std::env::set_var` is `unsafe` (forbidden in this
    /// workspace), so the child state is established the way a real child gets it: by re-executing
    /// this test binary with the variable in its environment, running [`llama_child_probe`] as the
    /// body. The child runs under a deadline and is killed when it passes it, so a wedged child
    /// fails this test instead of hanging the suite.
    #[test]
    fn a_subagent_child_treats_llama_like_the_other_ambient_natives() {
        use std::io::Read as _;
        use std::process::Stdio;

        const DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);
        for probe in ["keep", "no-extensions"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "--ignored",
                    "session_launch::tests::llama_child_probe",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_ENV, "1")
                .env(PROBE_ENV, probe)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            // Drained on their own threads, so a chatty child cannot fill a pipe and stall.
            let drain = |mut pipe: Box<dyn std::io::Read + Send>| {
                std::thread::spawn(move || {
                    let mut text = String::new();
                    let _ = pipe.read_to_string(&mut text);
                    text
                })
            };
            let stdout = drain(Box::new(child.stdout.take().unwrap()));
            let stderr = drain(Box::new(child.stderr.take().unwrap()));
            let started = std::time::Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break Some(status);
                }
                if started.elapsed() > DEADLINE {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            };
            let stdout = stdout.join().unwrap();
            let stderr = stderr.join().unwrap();
            assert!(
                status.is_some(),
                "child probe `{probe}` did not finish within {DEADLINE:?} and was killed\n\
                 --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
            );
            assert!(
                status.is_some_and(|s| s.success())
                    && stdout.contains(&format!("llama-probe-ok:{probe}")),
                "child probe `{probe}` failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
            );
        }
    }

    /// The body of the child probe above. `#[ignore]`d, so an ordinary run does not count a vacuous
    /// pass: only the parent test runs it (it passes `--ignored`), in a re-executed binary with the
    /// child marker in its environment. It is still a no-op unless [`PROBE_ENV`] is set (a person
    /// running the ignored tests by hand), and the parent insists on the success marker this prints,
    /// so a filter that matched nothing cannot pass for a probe that ran.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "re-executed by a_subagent_child_treats_llama_like_the_other_ambient_natives"]
    async fn llama_child_probe() {
        let Ok(probe) = std::env::var(PROBE_ENV) else {
            return;
        };
        assert!(
            std::env::var_os(CHILD_ENV).is_some(),
            "the probe must run as a subagent child"
        );
        let no_extensions = probe == "no-extensions";
        let Loaded { loaded, .. } = session_through_build_factory(false, no_extensions).await;
        let llama = loaded.iter().any(|i| i == LLAMA_ID);
        let mcp = loaded.iter().any(|i| i == MCP_ID);
        assert_eq!(
            llama, !no_extensions,
            "child, no_extensions={no_extensions}: llama.cpp is attached unless the flag drops it; \
             got {loaded:?}"
        );
        assert_eq!(
            llama, mcp,
            "child: llama.cpp follows the MCP adapter's ambient treatment; got {loaded:?}"
        );
        println!("llama-probe-ok:{probe}");
    }

    #[test]
    fn scoped_active_model_prefers_saved_default_in_scope_else_first() {
        // Pi `buildSessionOptions` (main.ts:394-414): saved default in scope wins; else scoped[0].
        let a = scoped("anthropic", "");
        let o = scoped("openai", "");
        let scope = vec![a.clone(), o.clone()];

        // Saved default IS in scope (case-insensitive) → it is chosen, even though it is not first.
        let picked = pick_scoped_active_model(
            &scope,
            Some(&o.model.provider.as_str().to_uppercase()),
            Some(&o.model.id.as_str().to_uppercase()),
        )
        .expect("a pick");
        assert_eq!(picked.model.provider, o.model.provider);
        assert_eq!(picked.model.id, o.model.id);

        // Saved default NOT in scope → fall back to the first scoped model.
        let picked =
            pick_scoped_active_model(&scope, Some("together"), Some("nope")).expect("a pick");
        assert_eq!(picked.model.provider, a.model.provider);

        // No saved default → the first scoped model.
        let picked = pick_scoped_active_model(&scope, None, None).expect("a pick");
        assert_eq!(picked.model.provider, a.model.provider);

        // An empty scope yields nothing to pick.
        assert!(pick_scoped_active_model(&[], Some("openai"), Some("gpt-4o")).is_none());
    }
}
