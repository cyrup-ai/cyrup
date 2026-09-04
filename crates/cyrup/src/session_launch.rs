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
/// order is load-bearing and unchanged:
///
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
    if let Some(runtime) = cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_for_env() {
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
    let mut builder = SessionFactory::new(provider, config)
        .settings_store(settings_store)
        .auth(auth_store)
        .trust_store(crate::prelaunch::trust_store_for(dirs));
    if let Some(prompt) = trust_prompt {
        builder = builder.trust_prompt(prompt);
    }
    builder = builder.provider_resolver(Arc::new(crate::provider::BuiltinProviderResolver::new(
        models_json,
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
}

/// Create the runtime, run pi's post-creation diagnostics checkpoint, apply the post-build knobs,
/// and apply the mode-gated modelless stop.
///
/// [`ControlFlow::Break`] carries the exit code the caller must return; [`ControlFlow::Continue`]
/// carries the live runtime + session.
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
) -> anyhow::Result<ControlFlow<i32, (Arc<AgentSessionRuntime>, Arc<AgentSession>)>> {
    timings::time("createRuntime", timings::TimingLabel::Main);
    let runtime = AgentSessionRuntime::create_unannounced(factory, target)
        .await
        .context("building agent session runtime")?;
    timings::time("createAgentSessionRuntime", timings::TimingLabel::Main);

    // Pi main.ts:843-848 (SEAM-S01) — report the runtime's build diagnostics and exit 1 on any
    // error (today: the extension-flag reconciliation errors and the extension LOAD failures).
    // Same checkpoint, every mode.
    if diagnostics::report_runtime(&runtime).await {
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
    Ok(ControlFlow::Continue((runtime, session)))
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
