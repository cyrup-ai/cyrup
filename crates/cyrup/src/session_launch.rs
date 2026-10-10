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
use cyrup_config::{AuthStore, ConfigDirs, ModelFile, SettingsManager, SettingsStore};
use cyrup_provider::Provider;
use cyrup_resources::package::manifest::BUILTIN_PATH_PREFIX;
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
///    (`extensions/index.ts:7-8`). Attached in every mode and inside a subagent child, exactly as
///    upstream has it in every session — `llama_extension_for_env` never gates — unless the
///    `extensions` setting disables `builtin:llama.cpp` ([`BuiltinSelection`], EXT-094).
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
///    (R-SA-020). That gate is resolved BEFORE the tier-3 `config.json` is read, matching the
///    order upstream's `registerSubagentExtension` has (the child return at
///    `pi-subagents/src/extension/index.ts:315-317`, then `loadConfig()` at `:325` @v0.75.0): a
///    process that registers nothing never opens the file, so a refusal of it
///    ([`SubagentAttachment::Refused`]) can only reach a process that WOULD have read it, where it
///    quarantines this one extension ([`quarantine`]).
/// 3. **The subagent prompt runtime** (SUBA-S01, pi `pi-args.ts:13`, which loads
///    `subagent-prompt-runtime.ts` into the child as its OWN extension): a plain subagent child
///    attaches NO subagents extension — `subagent_extension_for_env` returns `None` for it by
///    design — so the child-side `structured_output` tool cannot come from that gate. This one is
///    independent: it builds for a step that actually declared an `outputSchema`, i.e. when the
///    parent passed both structured-output env vars — `CYRUP_SUBAGENT_STRUCTURED_OUTPUT_SCHEMA`
///    naming a schema DOCUMENT on disk, not carrying one inline — and it also builds for a
///    fan-out-authorized child that passed neither. Both observed by
///    [`tests::llama_child_probe`], whose `plain` arm loads exactly `[llama.cpp, mcp]`, whose
///    `fanout` arm adds `subagents` AND `subagent-prompt-runtime`, and whose `structured` arm adds
///    `subagent-prompt-runtime` alone (EXT-101). Every other process attaches nothing.
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
///
/// The four natives pi ships as `builtin:<name>` extension resources — llama.cpp, codemode,
/// tool-search and mcp — attach only when `builtins` selects them (EXT-094): see
/// [`BuiltinSelection`].
fn attach_native_extensions(
    mut builder: SessionFactory,
    dirs: &ConfigDirs,
    session_cwd: PathBuf,
    builtins: &BuiltinSelection,
) -> anyhow::Result<SessionFactory> {
    let agent_dir: &Path = &dirs.agent_dir;
    // `-e builtin:<name>` naming no built-in: pi's loader records `Unknown built-in extension:
    // builtin:<name>` against that path (`loadExtensionPaths`, `core/resource-loader.ts`). An
    // explicit `-e` path survives `--no-extensions`, so the placeholder is inline-tier.
    for name in &builtins.unknown {
        let path = format!("{BUILTIN_PATH_PREFIX}{name}");
        builder = quarantine(
            builder,
            &path,
            cyrup_ext::QuarantinedTier::Inline,
            &format!("Unknown built-in extension: {path}"),
        );
    }
    if builtins.attaches(cyrup_llama::LLAMA_PROVIDER_ID) {
        if builtins.is_explicit(cyrup_llama::LLAMA_PROVIDER_ID) {
            builder = builder.with_native_extension(Arc::new(
                cyrup_llama::LlamaExtension::new(agent_dir.to_path_buf()).loaded_explicitly(),
            ));
        } else if let Some(ext) = cyrup_llama::llama_extension_for_env(agent_dir) {
            builder = builder.with_native_extension(ext);
        }
    }
    // The bundled worked virtual-model router ([`crate::router_example`]), behind
    // `CYRUP_ROUTER_EXAMPLE`. Gated OFF by default and attached the same way `llama` is, so an
    // existing session's catalog is byte-for-byte unchanged unless the user asks for a router.
    // Without it the virtual-model feature has no in-tree registrant and is unreachable from the
    // shipped binary — pi ships its equivalent as `examples/extensions/jev-router.ts`, which cyrup
    // has no place for (natives are compile-time and there is no `examples/` directory).
    if let Some(ext) = crate::router_example::router_extension_for_env() {
        builder = builder.with_native_extension(ext);
    }
    // `codemode` is the next entry of pi's `builtInExtensions` (`extensions/index.ts:9-14`
    // @v1.0.1), registered inactive and replaceable. Its scripts run in V8 isolates
    // ([`cyrup_codemode_runtime::tool::EngineSandboxFactory`], ADR-0031).
    //
    // `-e builtin:codemode` attaches it but cannot keep it under `--no-extensions`: its ambient
    // tier is fixed in `cyrup-codemode-runtime`, which has no explicit-load constructor.
    if builtins.attaches(cyrup_codemode_runtime::EXTENSION_ID) {
        builder = builder.with_codemode(cyrup_codemode_runtime::CodemodeExtension::new(
            cyrup_codemode_runtime::tool::CodemodeHostSlot::new(),
            Arc::new(cyrup_codemode_runtime::tool::EngineSandboxFactory),
        ));
    }
    // `tool_search` follows it (`extensions/index.ts:12` @v1.0.1): registered inactive and
    // replaceable, it loads `codemode` and `deferred` tools into the active set on request.
    if builtins.attaches(cyrup_tool_search::EXTENSION_ID) {
        let ext = cyrup_tool_search::ToolSearchExtension::new();
        builder = builder.with_native_extension(Arc::new(
            if builtins.is_explicit(cyrup_tool_search::EXTENSION_ID) {
                ext.loaded_explicitly()
            } else {
                ext
            },
        ));
    }
    // A malformed `intercom/config.json`, or an unusable `PI_INTERCOM_ASK_TIMEOUT_MS`, REFUSES
    // this extension. Both of upstream's equivalents throw from the first two lines of the
    // extension factory itself — `loadConfig()` (`pi-intercom/index.ts:648` @v0.16.0, throwing from
    // `config.ts:98-159`) and `getAskTimeoutMs()` (`:649`, throwing from `config.ts:18`) — so pi's
    // loader catches them, discards this one extension and builds the session without it. The
    // refusal therefore QUARANTINES the one extension, exactly as the two below do; carrying it out
    // of this function aborted the whole launch instead, and not one of the other six built-ins was
    // attempted.
    //
    // Ordering: this must precede `subagent_attachment`, because the `Configured` arm reads
    // `intercom_ext` to wire the subagent extension's delivery/clarify/steer channels. A
    // quarantined intercom leaves it `None`, so the subagent extension attaches channel-less —
    // which is what it does whenever intercom is absent, and is what upstream's subagent extension
    // finds when intercom failed to load.
    let intercom_ext = match cyrup_intercom::intercom_extension_for_env_concrete(
        agent_dir.to_path_buf(),
        session_cwd.clone(),
    ) {
        Ok(ext) => ext,
        Err(refusal) => {
            builder = quarantine(
                builder,
                cyrup_intercom::EXTENSION_ID,
                // `IntercomExtension::is_ambient` is `true` (`cyrup-intercom/src/extension.rs:655`):
                // it stands in for an installed package in the tier `--no-extensions` collapses, so
                // the placeholder must be dropped by that flag too.
                cyrup_ext::QuarantinedTier::Ambient,
                &refusal,
            );
            None
        }
    };
    // SUBA-166: a `config.json` that failed validation while declaring a policy key
    // (`authorityPolicy`, `permissions`, the route-identity and worktree keys) is REFUSED rather
    // than replaced by the all-defaults config — pi's `loadConfig` rethrows exactly those files
    // (`pi-subagents/src/extension/config.ts:226-240` @v0.75.0). The refusal QUARANTINES the one
    // extension: see [`quarantine`].
    let subagent_gate =
        cyrup_ext_subagents::extension::registration_mode_for_env(agent_dir, &session_cwd);
    let subagent_ext = match subagent_attachment(dirs, subagent_gate) {
        SubagentAttachment::Unregistered => None,
        SubagentAttachment::Refused(refusal) => {
            builder = quarantine(
                builder,
                cyrup_ext_subagents::extension::EXTENSION_ID,
                cyrup_ext::QuarantinedTier::Ambient,
                &refusal,
            );
            None
        }
        SubagentAttachment::Configured(config) => match &intercom_ext {
            Some(ic) => cyrup_ext_subagents::extension::subagent_extension_for_env_with_channels(
                agent_dir,
                *config,
                session_cwd.clone(),
                ic.delivery_channel(),
                ic.clarify_channel(),
                ic.steer_channel(),
            ),
            None => cyrup_ext_subagents::extension::subagent_extension_for_env(
                agent_dir,
                *config,
                session_cwd.clone(),
            ),
        },
    };
    if let Some(ext) = subagent_ext {
        builder = builder.with_native_extension(ext);
    }
    // CFG-080: the prompt runtime REFUSES to build when the tool budget the parent shipped in
    // `CYRUP_SUBAGENT_TOOL_BUDGET` does not decode — pi's `decodeToolBudgetEnv` throws out of
    // `registerSubagentPromptRuntime` (`pi-subagents` v0.64.0
    // `src/runs/shared/subagent-prompt-runtime.ts:693`, `tool-budget.ts:74-80`) and its loader
    // discards the extension. The refusal QUARANTINES the one extension here too, which is that
    // discard; the message is pi's own.
    let prompt_runtime =
        match cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_for_env() {
            Ok(runtime) => runtime,
            Err(refusal) => {
                builder = quarantine(
                    builder,
                    cyrup_ext_subagents::prompt_runtime::PROMPT_RUNTIME_EXTENSION_ID,
                    cyrup_ext::QuarantinedTier::Ambient,
                    &refusal,
                );
                None
            }
        };
    if let Some(runtime) = prompt_runtime {
        builder = builder.with_native_extension(runtime);
    }
    if let Some(ic) = intercom_ext {
        builder = builder.with_native_extension(ic);
    }
    if builtins.attaches(cyrup_mcp::EXTENSION_ID) {
        if builtins.is_explicit(cyrup_mcp::EXTENSION_ID) {
            let mcp_dirs =
                cyrup_mcp::dirs::McpDirs::new(agent_dir.to_path_buf(), session_cwd.clone());
            builder = builder.with_native_extension(
                cyrup_mcp::McpExtension::new(mcp_dirs)
                    .loaded_explicitly()
                    .into_arc(),
            );
        } else if let Some(ext) =
            cyrup_mcp::mcp_extension_for_env(agent_dir, None, session_cwd.clone())
        {
            builder = builder.with_native_extension(ext);
        }
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

/// Replace the native built-in `id` with [`cyrup_ext::QuarantinedNative`] carrying `reason`, so a
/// built-in that REFUSED to be built costs this launch that one extension and nothing else.
///
/// # Upstream's blast radius, which this restores
///
/// pi runs every extension factory inside a `try`. A factory that throws is `load.discard()`ed,
/// the loader records `Failed to load extension: <message>` and the loop CONTINUES to the next
/// extension (`initializeExtension` + `loadExtension`'s catch,
/// `pi/packages/coding-agent/src/core/extensions/loader.ts:613-630`, `:655` @v1.0.1; the
/// inline tier has the same catch at `core/resource-loader.ts:1130-1141`). The session is built,
/// without that extension; `main.ts:914-922` then reports the recorded error and exits 1.
///
/// Both of cyrup's refusing built-ins — the subagents extension whose `config.json` declared a
/// policy it will not run without (SUBA-166) and the subagent prompt runtime whose tool-budget
/// payload does not decode (CFG-080) — decline BEFORE the extension object exists, so there is no
/// factory for the loader to catch. Carrying the error out of this function instead (which is what
/// both arms did) aborted the entire launch: no session, and not one of the other six built-ins
/// attempted. That traded a fail-open for a strictly worse failure, since an ordinary typo in a
/// hand-edited `config.json` is not evidence of a tampered environment.
///
/// The placeholder is that missing throw. It registers nothing — `discard()` is therefore exact —
/// and fails `init`, which is the ONE path `cyrup-session-svc`'s build loop already treats as pi's
/// per-extension load failure (EXT-S01): the diagnostic lands on `StartupDiagnostics::extensions`
/// for the interactive `[Extension issues]` panel AND on `AgentSessionRuntime::diagnostics()` as a
/// fatal `Failed to load extension "<id>": <reason>`, which the bin reports and exits 1 on in
/// every mode. So the refusal still refuses — SUBA-166's config is still not loaded and CFG-080's
/// child still does not run — and it now refuses the way upstream does, through the same channel
/// every other extension-load failure already uses, with the other built-ins still attached.
fn quarantine(
    builder: SessionFactory,
    id: &str,
    tier: cyrup_ext::QuarantinedTier,
    reason: &dyn std::fmt::Display,
) -> SessionFactory {
    let reason = reason.to_string();
    tracing::error!(
        extension = %id,
        reason = %reason,
        "native built-in quarantined: it refused to be built"
    );
    builder.with_native_extension(Arc::new(cyrup_ext::QuarantinedNative::new(
        cyrup_core::ExtensionId::from(id),
        reason,
        tier,
    )) as Arc<dyn cyrup_ext::NativeExtension>)
}

/// What this process attaches for the SubAgents built-in — the three outcomes upstream's
/// registration distinguishes, named rather than folded into one `Option` whose `None` would mean
/// two unrelated things (`docs/RUST-DESIGN-REVIEW.md`, "Explicit domain enums").
enum SubagentAttachment {
    /// The gate declined: a plain subagent child, or a top-level session that never opted in.
    /// Upstream's `registerSubagentExtension` returns on the child flag
    /// (`pi-subagents/src/extension/index.ts:315-317` @v0.75.0) BEFORE it calls `loadConfig()`
    /// (`:325`), so such a process never opens `config.json` and can never be refused over it.
    /// That ordering is why this variant exists at all rather than being `Configured`'s absence.
    Unregistered,
    /// The gate passed and the tier-3 `config.json` loaded (or was absent, which is the
    /// all-defaults case). Build the extension with it.
    ///
    /// `Box`ed because the config is ~1.6 KiB and the other two variants are tiny — every
    /// `SubagentAttachment` the launch path moves would otherwise carry that much stack
    /// (`clippy::large_enum_variant`).
    Configured(Box<cyrup_ext_subagents::registration::SubagentExtensionConfig>),
    /// SUBA-166: the gate passed, and the `config.json` was REFUSED — it failed validation while
    /// declaring a policy key, so the built-in declines to run with the defaults in its place.
    Refused(crate::subagent_config::RefusedSubagentConfig),
}

/// Resolve [`SubagentAttachment`]: the attach `gate` is consulted FIRST and the tier-3
/// `config.json` is read only when it passed — upstream's order
/// (`pi-subagents/src/extension/index.ts:315-317` then `:325` @v0.75.0), and the reason
/// [`SubagentAttachment::Unregistered`] is a variant of its own rather than `Configured`'s absence.
///
/// `gate` is passed in rather than read from the environment here so the ordering is provable
/// without a process-global `std::env::set_var` (which is `unsafe`, and forbidden in this
/// workspace): the one caller supplies
/// [`cyrup_ext_subagents::extension::registration_mode_for_env`].
fn subagent_attachment(
    dirs: &ConfigDirs,
    gate: Option<cyrup_ext_subagents::extension::RegistrationMode>,
) -> SubagentAttachment {
    if gate.is_none() {
        return SubagentAttachment::Unregistered;
    }
    match crate::subagent_config::load_subagent_extension_config(dirs) {
        Ok(config) => SubagentAttachment::Configured(Box::new(config)),
        Err(refusal) => SubagentAttachment::Refused(refusal),
    }
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
    mut config: SessionConfig,
    settings_store: Arc<dyn SettingsStore>,
    auth_store: Arc<AuthStore>,
    dirs: &ConfigDirs,
    models_json: Arc<ModelFile>,
    trust_prompt: Option<TrustPromptFn>,
) -> anyhow::Result<Arc<SessionFactory>> {
    let session_cwd = config.cwd.clone();
    let builtins = BuiltinSelection::resolve(&mut config, &settings_store, dirs);
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
        &builtins,
    )?))
}

/// The names pi's `builtInExtensions` gives its built-in extensions (`extensions/index.ts`), each
/// an extension resource `builtin:<name>`. cyrup's natives for them load under the same ids.
pub const BUILTIN_EXTENSIONS: [&str; 4] = [
    cyrup_llama::LLAMA_PROVIDER_ID,
    cyrup_codemode_runtime::EXTENSION_ID,
    cyrup_tool_search::EXTENSION_ID,
    cyrup_mcp::EXTENSION_ID,
];

/// Which built-in extensions this launch attaches (EXT-094).
///
/// pi resolves each built-in as an extension resource (`DefaultPackageManager.resolve` and
/// `resolveExtensionSources`, `core/package-manager.ts`): it loads by default, a `-builtin:<name>`
/// (or `!` glob) in the `extensions` setting disables it, and `-e builtin:<name>` loads it
/// explicitly — into `cliEnabledExtensions`, which wins over the setting and which
/// `--no-extensions` keeps (`core/resource-loader.ts`). cyrup decides the first two here, at
/// attach; `--no-extensions` stays the builder's `is_ambient` gate, so an explicit built-in is
/// attached as a non-ambient one.
///
/// Only the USER `extensions` array is read, once per factory. pi lets a project `+`/`-`/`!` entry
/// override it and re-resolves on every `resolve()` (so `/reload` sees a changed setting), but
/// project settings exist only once project trust is resolved, and cyrup resolves that inside the
/// session build, after the natives are attached.
#[derive(Debug, Default)]
struct BuiltinSelection {
    /// Built-ins `-e builtin:<name>` named.
    explicit: Vec<String>,
    /// `-e builtin:<name>` names that are no built-in.
    unknown: Vec<String>,
    /// Built-ins the user `extensions` setting disables.
    disabled: Vec<&'static str>,
}

impl BuiltinSelection {
    /// Take the `builtin:` entries out of `config.extra_extension_paths` — they name no file for
    /// the discovery the builder runs over that list — and read the user `extensions` setting.
    fn resolve(
        config: &mut SessionConfig,
        settings_store: &Arc<dyn SettingsStore>,
        dirs: &ConfigDirs,
    ) -> Self {
        let mut selection = Self::default();
        config.extra_extension_paths.retain(|path| {
            let Some(name) = path
                .to_str()
                .and_then(|p| p.strip_prefix(BUILTIN_PATH_PREFIX))
            else {
                return true;
            };
            let bucket = if BUILTIN_EXTENSIONS.contains(&name) {
                &mut selection.explicit
            } else {
                &mut selection.unknown
            };
            if !bucket.iter().any(|n| n == name) {
                bucket.push(name.to_string());
            }
            false
        });
        let user = SettingsManager::load(Arc::clone(settings_store), false)
            .global()
            .extension_paths();
        selection.disabled = BUILTIN_EXTENSIONS
            .into_iter()
            .filter(|name| {
                !cyrup_resources::package::manifest::builtin_extension_enabled(
                    name,
                    &user,
                    &dirs.agent_dir,
                    &[],
                    &dirs.cwd,
                )
            })
            .collect();
        selection
    }

    /// Whether the built-in `name` is attached: named by `-e`, or not disabled by the setting.
    fn attaches(&self, name: &str) -> bool {
        self.is_explicit(name) || !self.disabled.contains(&name)
    }

    /// Whether `-e builtin:<name>` named it, which keeps it under `--no-extensions`.
    fn is_explicit(&self, name: &str) -> bool {
        self.explicit.iter().any(|n| n == name)
    }
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
    // SEAM-147 — pi resolves the scope and the CLI model BEFORE deciding `--api-key`
    // (`main.ts:812-838` @f1b2e77f5): the key needs `sessionOptions.model`, which only `--model` or
    // a fresh session's scope pick sets, and it goes to THAT model's provider. The `--model` /
    // `--provider` key is installed by `main.rs` before the build; the scope pick's is installed
    // here, before the pick is applied, because `set_model_resolved` runs pi's `hasConfiguredAuth`
    // precheck. pi raises the missing-model case as an error diagnostic, reported with the
    // runtime's and fatal (`:902-908`), so it sits here, after the `--help` / `--list-models` exits.
    let scope = resolve_launch_scope(&session, post.cli, post.fresh);
    if let Some(key) = post.cli.api_key.as_deref() {
        match api_key_target(post.cli, &scope) {
            ApiKeyTarget::FromCli => {}
            ApiKeyTarget::Scoped(provider) => {
                session
                    .services()
                    .auth
                    .set_runtime_api_key(provider, key.to_string());
            }
            ApiKeyTarget::Missing => {
                diagnostics::report(&[Diagnostic::error(API_KEY_REQUIRES_MODEL)]);
                runtime.dispose().await;
                crate::output_guard::restore_stdout();
                return Ok(ControlFlow::Break(1));
            }
        }
    }
    apply_post_build(&session, post.session_name, post.cli, scope).await;

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

/// pi's `--api-key` error text (`main.ts:833` @f1b2e77f5).
pub const API_KEY_REQUIRES_MODEL: &str =
    "--api-key requires a model to be specified via --model, --provider/--model, or --models";

/// The `--models` / `enabledModels` scope a launch resolved, and the model it picks for a fresh
/// session (pi `scopedModels` and `buildSessionOptions`' scope arm, `main.ts:812-815`, `:499-519`
/// @f1b2e77f5).
pub(crate) struct LaunchScope {
    scoped: Vec<ScopedModel>,
    /// pi `options.model` from the scope: set only with no `--model`, a non-empty scope and no
    /// existing session.
    pick: Option<ScopedModel>,
}

/// Resolve the launch scope, printing pi's scope warnings.
///
/// The patterns follow pi's precedence `parsed.models ?? settingsManager.getEnabledModels()`: an
/// explicit `--models` wins, even empty, otherwise the persisted `enabledModels` setting. They
/// resolve against [`AgentSession::available_model_catalog`] — pi's `resolveModelScope` runs over
/// `modelRuntime.getAvailable()` (`core/model-resolver.ts:364-370`), every provider with configured
/// auth — not against the installed provider's own catalog: SEAM-147 found that a `--models` naming
/// any provider other than the installed one (and, with the zero-model placeholder installed, any
/// provider at all) matched nothing. Matching itself is `cyrup-config`'s `minimatch`-faithful
/// resolver (see [`resolve_scoped_models_reporting`]).
///
/// `fresh` is pi's `!hasExistingSession`: a resumed session keeps its own restored model, so the
/// saved-default-in-scope pick fires only for a fresh one.
pub(crate) fn resolve_launch_scope(session: &AgentSession, cli: &Cli, fresh: bool) -> LaunchScope {
    let patterns = scope_patterns(cli.models.as_deref(), || {
        session.services().settings.effective().enabled_models()
    });
    if patterns.is_empty() {
        return LaunchScope {
            scoped: Vec::new(),
            pick: None,
        };
    }
    let catalog = session.available_model_catalog();
    // Pi `resolveModelScope` prints EVERY diagnostic its `WithDiagnostics` sibling collected —
    // `console.warn(chalk.yellow(`Warning: ${diagnostic.message}`))`, model-resolver.ts:378-380 —
    // before returning the (possibly empty) scope. Without this a typo'd
    // `--models "anthropc/*"` scoped nothing with no output at all.
    let (scoped, diags) = resolve_scoped_models_reporting(&catalog, &patterns);
    diagnostics::report(&diags);
    let pick = if cli.model.is_none() && fresh {
        let eff = session.services().settings.effective();
        pick_scoped_active_model(
            &scoped,
            eff.default_provider().as_deref(),
            eff.default_model().as_deref(),
        )
        .cloned()
    } else {
        None
    };
    LaunchScope { scoped, pick }
}

/// Where a launch's `--api-key` goes (pi `main.ts:830-838` @f1b2e77f5).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ApiKeyTarget {
    /// `--model` / `--provider` named the model; `main.rs` installed the key on its provider.
    FromCli,
    /// The scope picked the model; the key goes to its provider.
    Scoped(cyrup_sdk::core::ProviderId),
    /// No model was resolved: pi's error diagnostic, exit 1.
    Missing,
}

/// pi tests the RESOLVED `sessionOptions.model`, not which flags were typed: `--api-key k --models
/// ""`, a scope matching nothing, and a scope on a resumed session all leave it unset and error,
/// while an `enabledModels` scope picking a model on a fresh session satisfies it with no
/// `--models` at all. cyrup resolves a model from `--provider` alone, so `--provider` counts as
/// naming one (pi refuses a bare `--provider` outright, `main.ts:468-473`).
pub(crate) fn api_key_target(cli: &Cli, scope: &LaunchScope) -> ApiKeyTarget {
    if cli.model.is_some() || cli.provider.is_some() {
        return ApiKeyTarget::FromCli;
    }
    match &scope.pick {
        Some(pick) => ApiKeyTarget::Scoped(pick.model.provider.clone()),
        None => ApiKeyTarget::Missing,
    }
}

/// Apply the per-run, post-build session knobs that have no `SessionConfig` slot: the trimmed
/// `--name` display name (Pi `appendSessionInfo`, main.ts:586) and the resolved scope — the
/// fresh-session active-model pick (Pi `buildSessionOptions`, `main.ts:499-519` @f1b2e77f5) and the
/// Ctrl+P cycle set.
async fn apply_post_build(
    session: &AgentSession,
    name: Option<&str>,
    cli: &Cli,
    scope: LaunchScope,
) {
    if let Some(name) = name {
        let _ = session.set_session_name(name).await;
    }
    let LaunchScope { scoped, pick } = scope;
    if scoped.is_empty() {
        return;
    }
    if let Some(chosen) = pick
        && session.set_model_resolved(chosen.model).await.is_ok()
        // Use the scoped model's thinking level only when `--thinking` was omitted (explicit
        // `--thinking` takes precedence and is applied by the builder).
        && cli.thinking.is_none()
        && let Some(level) = chosen.thinking_level
    {
        let _ = session.set_thinking_level(level).await;
    }
    session.set_scoped_models(scoped);
}

/// Pi `modelPatterns = parsed.models ?? settingsManager.getEnabledModels()` (`main.ts:812-815`
/// @f1b2e77f5): a SUPPLIED `--models` wins even when it is empty (`--models ""` is pi's truthy
/// `[]`, which then skips scoping); the `enabledModels` setting is read only when the flag is
/// absent. SEAM-147.
fn scope_patterns(
    cli_models: Option<&[String]>,
    enabled_models: impl FnOnce() -> Option<Vec<String>>,
) -> Vec<String> {
    match cli_models {
        Some(patterns) => patterns.to_vec(),
        None => enabled_models().unwrap_or_default(),
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

    /// SEAM-147 Verify — `--models "faux/*,"` resolves and warns exactly as `--models "faux/*"`,
    /// and `--models ""` still counts as a supplied flag and resolves no scope. pi `9b3c19da5`
    /// (`cli/args.ts:141-145` @v1.0.1) drops empty entries; `main.ts:812-815` @f1b2e77f5 reads
    /// `parsed.models ?? settingsManager.getEnabledModels()` and scopes only a non-empty list. The
    /// patterns go through the real clap parse and `normalize_list_flags`, then the same
    /// [`super::scope_patterns`] → [`resolve_scoped_models_reporting`] pair
    /// [`super::resolve_launch_scope`] runs. RED before the fix: the trailing comma added `No models match pattern ""`, and
    /// `--models ""` resolved the pattern `""` with that same warning. `anthropic/*` stands in for
    /// the Verify line's `faux/*`: `faux` is not in the bundled catalog this resolves against
    /// (`provider::tests::all_available_models_span_the_full_registry` asserts it absent), so
    /// `faux/*` would scope nothing on either side. What a supplied-but-empty scope does to the
    /// launch model and to `--api-key` is covered by `tests::api_key_models_scope`.
    #[test]
    fn a_trailing_comma_in_models_resolves_and_warns_like_the_bare_pattern() {
        use clap::Parser;
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());
        let patterns_for = |args: &[&str]| -> Vec<String> {
            let mut argv = vec!["cyrup"];
            argv.extend_from_slice(args);
            let mut cli = crate::cli::Cli::try_parse_from(argv).expect("parses");
            cli.normalize_list_flags();
            super::scope_patterns(cli.models.as_deref(), || Some(vec!["openai/*".to_string()]))
        };
        let messages = |patterns: &[String]| -> (Vec<String>, Vec<String>) {
            let (scoped, diags) = resolve_scoped_models_reporting(&catalog, patterns);
            (
                scoped
                    .iter()
                    .map(|s| format!("{}/{}", s.model.provider.as_str(), s.model.id.as_str()))
                    .collect(),
                diags.into_iter().map(|d| d.message).collect(),
            )
        };

        let bare = messages(&patterns_for(&["--models", "anthropic/*"]));
        assert!(!bare.0.is_empty(), "`anthropic/*` scopes something");
        assert!(bare.1.is_empty(), "{:?}", bare.1);
        assert_eq!(messages(&patterns_for(&["--models", "anthropic/*,"])), bare);
        assert_eq!(
            messages(&patterns_for(&["--models", " anthropic/* , ,"])),
            bare
        );

        // `--models ""`: supplied, so no `enabledModels` fallback, and nothing to scope or warn.
        let empty = patterns_for(&["--models", ""]);
        assert!(empty.is_empty(), "{empty:?}");
        assert_eq!(messages(&empty), (Vec::new(), Vec::new()));
        // Absent: the `enabledModels` setting is the scope source.
        assert_eq!(patterns_for(&[]), vec!["openai/*".to_string()]);
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

    use std::path::Path;
    use std::sync::Arc;

    use cyrup_provider::Provider;
    use cyrup_provider::faux::FauxProvider;
    use cyrup_session_svc::{
        AgentSession, AgentSessionRuntime, RuntimeDiagnostic, SessionConfig, SessionFactory,
        TrustPromptFn,
    };

    use super::{BUILTIN_EXTENSIONS, build_factory};

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

    /// The fan-out authorization a parent puts in a fan-out child's environment (documented at
    /// this module's item 2: `CYRUP_SUBAGENT_FANOUT_CHILD=1` gets the restricted, mutation-blocked
    /// subagent tool REGARDLESS of `is_installed`). EXT-101.
    const FANOUT_ENV: &str = "CYRUP_SUBAGENT_FANOUT_CHILD";

    /// The two variables a parent sets ONLY for a step that declared an `outputSchema`, and which
    /// the subagent prompt runtime needs BOTH of before it builds (this module's item 3). EXT-101.
    const STRUCTURED_SCHEMA_ENV: &str = "CYRUP_SUBAGENT_STRUCTURED_OUTPUT_SCHEMA";
    const STRUCTURED_CAPTURE_ENV: &str = "CYRUP_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE";

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

    /// The factory a mode arm would build over `agent_dir` + `cwd`, and the target to build it at.
    /// Shared by [`session_through_build_factory`] (which is handed a throwaway home) and by the
    /// quarantine tests, which have to PREPARE that home — write a `subagents/config.json` into it
    /// — before the factory is built.
    fn factory_at(
        agent_dir: &Path,
        cwd: &Path,
        interactive: bool,
        no_extensions: bool,
    ) -> (Arc<SessionFactory>, cyrup_session_svc::SessionTarget) {
        factory_over(
            Arc::new(FauxProvider::new()),
            agent_dir,
            cwd,
            interactive,
            no_extensions,
        )
    }

    /// [`factory_at`] over a provider the test scripts.
    fn factory_over(
        provider: Arc<dyn Provider>,
        agent_dir: &Path,
        cwd: &Path,
        interactive: bool,
        no_extensions: bool,
    ) -> (Arc<SessionFactory>, cyrup_session_svc::SessionTarget) {
        factory_with(provider, agent_dir, cwd, interactive, |config| {
            config.no_extensions = no_extensions;
        })
    }

    /// [`factory_over`] with the per-run [`SessionConfig`] knobs set by `configure`.
    fn factory_with(
        provider: Arc<dyn Provider>,
        agent_dir: &Path,
        cwd: &Path,
        interactive: bool,
        configure: impl FnOnce(&mut SessionConfig),
    ) -> (Arc<SessionFactory>, cyrup_session_svc::SessionTarget) {
        let env = cyrup_config::EnvVars {
            home: Some(agent_dir.to_path_buf()),
            ..cyrup_config::EnvVars::default()
        };
        let overrides = cyrup_config::CliConfigOverrides {
            agent_dir: Some(agent_dir.to_path_buf()),
            cwd: Some(cwd.to_path_buf()),
            ..Default::default()
        };
        let dirs = cyrup_config::ConfigDirs::resolve(&overrides, &env).unwrap();

        let mut config = SessionConfig::new(cwd.to_path_buf(), agent_dir.to_path_buf());
        config.persist = false;
        config.trust_override = Some(true);
        configure(&mut config);
        let target = config.target.clone();

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
        (factory, target)
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
        let (factory, target) = factory_at(&agent_dir, &cwd, interactive, no_extensions);
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

    /// EXT-092 — every native `attach_native_extensions` loads has a DECIDED hidden state, and it
    /// is the one pi's load path gives its counterpart.
    ///
    /// pi infers hidden from the load path: `extension.hidden = true` only in the `builtin:<name>`
    /// branch of `loadExtensionPaths` (`core/resource-loader.ts:741` @f1b2e77f5), and the four
    /// `builtin: true` entries are `llama.cpp`, `codemode`, `tool-search` and `mcp`
    /// (`extensions/index.ts:7-13`). Everything else reaches pi as a file or package path and is
    /// listed. cyrup has no `builtin:<name>` path tier — every compiled-in native, built-in
    /// stand-in or package port alike, goes through the one `with_native_extension` door — so the
    /// native declares `NativeExtension::is_hidden` and THIS table is what holds the declaration
    /// to pi. A native added to `attach_native_extensions` without a row here fails the test, which
    /// forces the decision; a stand-in for a pi built-in that forgets the override is listed and
    /// fails it too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_attached_native_is_hidden_exactly_when_pi_loads_its_counterpart_as_a_builtin() {
        // (id, hidden, pi counterpart). `true` only for a stand-in for a `builtin: true` entry.
        let decided: &[(&str, bool, &str)] = &[
            (LLAMA_ID, true, "builtin:llama.cpp"),
            (
                cyrup_codemode_runtime::EXTENSION_ID,
                true,
                "builtin:codemode",
            ),
            (cyrup_tool_search::EXTENSION_ID, true, "builtin:tool-search"),
            // A port of the `pi-mcp-adapter` package (MCP-587), loaded by package path and listed;
            // pi's own `builtin:mcp` is not what this is.
            (MCP_ID, false, "package pi-mcp-adapter"),
            ("cyrup-flux", false, "cyrup's flux pipeline; no pi built-in"),
            (cyrup_intercom::EXTENSION_ID, false, "package pi-intercom"),
            (
                cyrup_ext_subagents::extension::EXTENSION_ID,
                false,
                "package pi-subagents",
            ),
            (
                cyrup_ext_subagents::prompt_runtime::PROMPT_RUNTIME_EXTENSION_ID,
                false,
                "pi-subagents' subagent-prompt-runtime.ts, loaded by path",
            ),
            (
                cyrup_permission_system::extension::EXTENSION_ID,
                false,
                "package pi-permission-system",
            ),
            (
                crate::router_example::EXTENSION_ID,
                false,
                "examples/extensions/jev-router.ts, loaded by path",
            ),
        ];
        let Loaded { loaded, listed } = session_through_build_factory(true, false).await;
        for id in &loaded {
            let Some((_, hidden, pi)) = decided.iter().find(|(d, ..)| d == id) else {
                panic!(
                    "native {id:?} is attached but has no hidden decision here: decide whether pi \
                     loads its counterpart as a `builtin:` (hidden) and add a row; loaded {loaded:?}"
                );
            };
            assert_eq!(
                !listed.contains(id),
                *hidden,
                "{id} (pi: {pi}) must be {} the startup [Extensions] list; listed {listed:?}",
                if *hidden { "absent from" } else { "in" }
            );
        }
        for (id, hidden, _) in decided {
            if *hidden {
                position(&loaded, id);
            }
        }
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

    /// TOOL-052 — `tool_search` is attached by `attach_native_extensions` (`extensions/index.ts:12`
    /// @v1.0.1) in every mode, registered inactive and `model-only`; `--no-extensions` drops it as
    /// it drops the other ambient built-ins. Read off a session built through `build_factory`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tool_search_is_attached_inactive_and_model_only_in_every_mode() {
        for (mode, interactive) in [("interactive", true), ("rpc/print/json", false)] {
            for no_extensions in [false, true] {
                let tmp = tempfile::tempdir().unwrap();
                let cwd = tmp.path().join("project");
                let agent_dir = tmp.path().join("agent");
                std::fs::create_dir_all(&cwd).unwrap();
                std::fs::create_dir_all(&agent_dir).unwrap();
                let (factory, target) = factory_at(&agent_dir, &cwd, interactive, no_extensions);
                let session: AgentSession = factory.build(target, None).await.unwrap();
                let row = session
                    .all_tools()
                    .into_iter()
                    .find(|row| row.name == "tool_search");
                if no_extensions {
                    assert!(row.is_none(), "{mode}: --no-extensions drops tool_search");
                    continue;
                }
                let row = row.unwrap_or_else(|| panic!("{mode}: tool_search is registered"));
                assert_eq!(row.exposure, cyrup_core::ToolExposure::ModelOnly, "{mode}");
                assert!(
                    !session
                        .active_tool_names()
                        .iter()
                        .any(|n| n == "tool_search"),
                    "{mode}: registered `defaultActive: false`"
                );
                assert!(
                    session
                        .services()
                        .ext_host
                        .loaded_ids()
                        .iter()
                        .any(|id| id.to_string() == cyrup_tool_search::EXTENSION_ID),
                    "{mode}: the extension is loaded"
                );
            }
        }
    }

    // ============================================================================================
    // MCP-604 — a `directTools: "search"` server's tools reach the model through `tool_search` and
    // through `mcp({ search })`, over the REAL launch wiring: the MCP adapter and `tool_search` are
    // the built-ins `attach_native_extensions` attaches, and the provider is scripted.
    // ============================================================================================

    /// The tool names each provider request declared, in request order.
    type Declared = Arc<std::sync::Mutex<Vec<Vec<String>>>>;

    /// A provider whose `n`th reply is `replies[n]` — a tool call (`Some((name, args))`) or text —
    /// recording the tool names every request declared.
    fn scripted(
        declared: &Declared,
        replies: Vec<Option<(&'static str, serde_json::Value)>>,
    ) -> Arc<FauxProvider> {
        let faux = Arc::new(FauxProvider::new());
        faux.set_response_steps(scripted_steps(declared, replies));
        faux
    }

    /// The response steps [`scripted`] installs.
    fn scripted_steps(
        declared: &Declared,
        replies: Vec<Option<(&'static str, serde_json::Value)>>,
    ) -> Vec<cyrup_provider::faux::FauxResponseStep> {
        use cyrup_core::StopReason;
        use cyrup_provider::faux::{
            FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
        };
        replies
            .into_iter()
            .map(|reply| {
                let seen = Arc::clone(declared);
                FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                    seen.lock()
                        .unwrap()
                        .push(ctx.tools.iter().map(|tool| tool.name.clone()).collect());
                    match &reply {
                        Some((name, args)) => faux_assistant_message(
                            vec![faux_tool_call((*name).to_string(), args.clone())],
                            StopReason::ToolUse,
                        ),
                        None => faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
                    }
                })
            })
            .collect()
    }

    /// `<agent_dir>/mcp.json` with one server `docs` in search mode, and an `mcp-cache.json` that
    /// is valid for it: two tools, so only one of them matches a query. No server is spawned.
    fn write_search_mode_world(agent_dir: &Path) {
        let definition = cyrup_mcp::config::ServerEntry {
            command: Some("echo".to_string()),
            ..cyrup_mcp::config::ServerEntry::default()
        };
        let hash = cyrup_mcp::registration::default_server_hasher(&definition).unwrap();
        std::fs::write(
            agent_dir.join("mcp.json"),
            serde_json::json!({
                "mcpServers": { "docs": { "command": "echo", "directTools": "search" } }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            agent_dir.join("mcp-cache.json"),
            serde_json::json!({
                "version": cyrup_mcp::registration::METADATA_CACHE_VERSION,
                "servers": { "docs": {
                    "configHash": hash,
                    "cachedAt": 4_102_444_800_000_u64,
                    "tools": [
                        {
                            "name": "lookup_widget",
                            "description": "Look up a widget by name.",
                            "inputSchema": {
                                "type": "object",
                                "properties": { "name": { "type": "string" } },
                                "required": ["name"]
                            }
                        },
                        { "name": "rotate_logs", "description": "Rotate the log files." }
                    ],
                    "resources": []
                } }
            })
            .to_string(),
        )
        .unwrap();
    }

    /// A session over [`write_search_mode_world`], built through `build_factory`, with `extra`
    /// activated on top of the default tools (`--tools`).
    async fn search_mode_session(
        provider: Arc<FauxProvider>,
        extra: &[&str],
    ) -> (Arc<AgentSessionRuntime>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        write_search_mode_world(&agent_dir);
        let (factory, target) = factory_over(provider, &agent_dir, &cwd, false, false);
        // The runtime, as every mode builds its session: it shares the session (which binds the
        // self-handle the turn-boundary refresh reads, so a mid-run load reaches the next request)
        // and announces `session_start`, which is what builds the MCP runtime.
        let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
        let session = runtime.session().await;
        let mut active = session.active_tool_names();
        active.extend(extra.iter().map(|name| (*name).to_string()));
        session.set_active_tools_by_name(&active).await;
        (runtime, tmp)
    }

    /// The registered tools of a search-mode server are at the `deferred` exposure, in the server's
    /// namespace, and not in the active set: the model is not shown them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_search_mode_servers_tools_are_registered_deferred_and_not_declared() {
        let declared: Declared = Arc::default();
        let (runtime, _tmp) = search_mode_session(scripted(&declared, vec![None]), &[]).await;
        let session = runtime.session().await;
        let rows = session.all_tools();
        let row = rows
            .iter()
            .find(|row| row.name == "docs_lookup_widget")
            .expect("registered");
        assert_eq!(row.exposure, cyrup_core::ToolExposure::Deferred);
        assert_eq!(
            row.namespace
                .as_ref()
                .map(|namespace| namespace.name.as_str()),
            Some("mcp__docs")
        );
        assert!(
            !session
                .active_tool_names()
                .iter()
                .any(|name| name.starts_with("docs_")),
            "{:?}",
            session.active_tool_names()
        );
        let _ = session.prompt("hi").await.unwrap();
        session.wait_for_idle().await;
        let requests = declared.lock().unwrap().clone();
        assert!(
            names_at(&requests, 0)
                .iter()
                .all(|name| !name.starts_with("docs_")),
            "a deferred tool is not in the request: {:?}",
            names_at(&requests, 0)
        );
        assert!(
            names_at(&requests, 0).iter().any(|name| name == "mcp"),
            "the gateway is: {:?}",
            names_at(&requests, 0)
        );
    }

    /// MCP-604 `verify`: a search-mode MCP tool is findable by `tool_search` while inactive and is
    /// activated by it; the very next request declares it, with the cached schema.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tool_search_finds_a_search_mode_mcp_tool_and_the_next_request_declares_it() {
        let declared: Declared = Arc::default();
        let provider = scripted(
            &declared,
            vec![
                Some(("tool_search", serde_json::json!({ "query": "widget" }))),
                None,
            ],
        );
        let (runtime, _tmp) = search_mode_session(provider, &["tool_search"]).await;
        let session = runtime.session().await;
        let _ = session.prompt("find the widget tool").await.unwrap();
        session.wait_for_idle().await;

        let requests = declared.lock().unwrap().clone();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(
            names_at(&requests, 0)
                .iter()
                .any(|name| name == "tool_search")
        );
        assert!(
            !names_at(&requests, 0)
                .iter()
                .any(|name| name == "docs_lookup_widget")
        );
        assert!(
            names_at(&requests, 1)
                .iter()
                .any(|name| name == "docs_lookup_widget"),
            "{:?}",
            names_at(&requests, 1)
        );
        assert!(
            !names_at(&requests, 1)
                .iter()
                .any(|name| name == "docs_rotate_logs"),
            "only the match is loaded: {:?}",
            names_at(&requests, 1)
        );
    }

    /// The gateway is the adapter's own entry point: a `mcp({ search })` that matches a search-mode
    /// tool loads it, so the very next request declares it, and the model is told so.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mcp_search_loads_a_search_mode_tool_for_the_next_request() {
        let declared: Declared = Arc::default();
        let provider = scripted(&declared, Vec::new());
        let (runtime, _tmp) = search_mode_session(Arc::clone(&provider), &[]).await;
        let session = runtime.session().await;

        // The MCP runtime builds in the background after `session_start`; a call that lands first
        // answers `not_initialized`. Ask for the gateway's status until it is up.
        let mut ready = false;
        for _ in 0..200 {
            provider.set_response_steps(scripted_steps(
                &declared,
                vec![Some(("mcp", serde_json::json!({}))), None],
            ));
            let _ = session.prompt("status").await.unwrap();
            session.wait_for_idle().await;
            let status = last_tool_result(&session, "mcp").await;
            if status
                .1
                .as_ref()
                .is_none_or(|details| details["error"] != "not_initialized")
            {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ready, "the MCP runtime never came up");
        declared.lock().unwrap().clear();

        provider.set_response_steps(scripted_steps(
            &declared,
            vec![
                Some(("mcp", serde_json::json!({ "search": "widget" }))),
                None,
            ],
        ));
        let _ = session.prompt("find the widget tool").await.unwrap();
        session.wait_for_idle().await;

        let requests = declared.lock().unwrap().clone();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(
            !names_at(&requests, 0)
                .iter()
                .any(|name| name == "docs_lookup_widget")
        );
        assert!(
            names_at(&requests, 1)
                .iter()
                .any(|name| name == "docs_lookup_widget"),
            "{:?}",
            names_at(&requests, 1)
        );
        assert!(
            !names_at(&requests, 1)
                .iter()
                .any(|name| name == "docs_rotate_logs")
        );
        let (content, details) = last_tool_result(&session, "mcp").await;
        let text = match content.first() {
            Some(cyrup_core::Content::Text { text, .. }) => text.to_string(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(
            text.starts_with("Activated as direct tools: docs_lookup_widget.\n\n"),
            "{text}"
        );
        assert_eq!(
            details.as_ref().unwrap()["activated"],
            serde_json::json!(["docs_lookup_widget"])
        );
    }

    /// The tool names request `index` declared (empty when there was no such request).
    fn names_at(requests: &[Vec<String>], index: usize) -> &[String] {
        requests.get(index).map_or(&[], Vec::as_slice)
    }

    /// The newest `tool_name` tool result of `session`: its content and details.
    async fn last_tool_result(
        session: &AgentSession,
        tool_name: &str,
    ) -> (Vec<cyrup_core::Content>, Option<serde_json::Value>) {
        session
            .messages()
            .await
            .into_iter()
            .rev()
            .find_map(|message| match message {
                cyrup_core::Message::ToolResult {
                    tool_name: name,
                    content,
                    details,
                    ..
                } if name == tool_name => Some((content, details)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("a `{tool_name}` tool result"))
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
    /// Re-execute THIS test binary to run the `#[ignore]`d test `probe` with `env` set, and insist
    /// that it both succeeded and printed `marker`.
    ///
    /// The environment is process-global and `std::env::set_var` is `unsafe` (forbidden in this
    /// workspace), so every launch decision that reads a variable is proven the way a real process
    /// gets it: in a child with that variable in its environment. The child runs under a deadline
    /// and is killed when it passes it, so a wedged child fails its test instead of hanging the
    /// suite; the marker is insisted on so a filter that matched NOTHING cannot pass for a probe
    /// that ran.
    fn run_env_probe(probe: &str, env: &[(&str, &str)], marker: &str) {
        use std::io::Read as _;
        use std::process::Stdio;

        const DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "--ignored",
            probe,
            "--nocapture",
            "--test-threads=1",
        ]);
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd
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
            "probe `{probe}` {env:?} did not finish within {DEADLINE:?} and was killed\n\
             --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        );
        assert!(
            status.is_some_and(|s| s.success()) && stdout.contains(marker),
            "probe `{probe}` {env:?} failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        );
    }

    #[test]
    fn a_subagent_child_treats_llama_like_the_other_ambient_natives() {
        // EXT-101: three child environments, not one. A child carrying ORCHESTRATOR METADATA is a
        // different launch — the fan-out authorization and the structured-output pair each turn on
        // another extension in `attach_native_extensions`, ahead of and after the llama.cpp line —
        // so "the marker alone" was one third of what a real child looks like. The probe asserts
        // that the extra extension really attached, so a renamed or misspelled variable fails this
        // test instead of quietly making the extra arms copies of the first.
        //
        // Held for the whole test: the structured-output pair names real files, and
        // `STRUCTURED_SCHEMA_ENV` is a PATH to a schema document, not an inline schema — an
        // inline one leaves the prompt runtime unbuilt, which the companion assertion in the probe
        // catches.
        let tmp = tempfile::tempdir().expect("a temp dir");
        let schema = tmp.path().join("output-schema.json");
        std::fs::write(&schema, r#"{"type":"object"}"#).expect("the schema document");
        let schema = schema.to_string_lossy().into_owned();
        let capture = tmp.path().join("structured-output.json");
        let capture = capture.to_string_lossy().into_owned();
        let flavours: [(&str, Vec<(&str, &str)>); 3] = [
            // The no-orchestrator-metadata case: the child marker alone.
            ("plain", vec![]),
            // A fan-out child: authorized for the restricted subagent tool.
            ("fanout", vec![(FANOUT_ENV, "1")]),
            // A child running a step that declared an `outputSchema`: both variables, or the
            // prompt runtime does not build.
            (
                "structured",
                vec![
                    (STRUCTURED_SCHEMA_ENV, schema.as_str()),
                    (STRUCTURED_CAPTURE_ENV, capture.as_str()),
                ],
            ),
        ];
        for (flavour, extra) in &flavours {
            for probe in ["keep", "no-extensions"] {
                let mut env = vec![(CHILD_ENV, "1"), (PROBE_ENV, probe)];
                env.extend(extra.iter().copied());
                run_env_probe(
                    "session_launch::tests::llama_child_probe",
                    &env,
                    &format!("llama-probe-ok:{flavour}:{probe}"),
                );
            }
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
        // The flavour is read from the environment the parent handed this process, not passed as a
        // label: a variable the parent misspells is then absent here, and the companion assertion
        // below fails instead of the arm silently degenerating into the `plain` one.
        let fanout = std::env::var_os(FANOUT_ENV).is_some();
        let structured = std::env::var_os(STRUCTURED_SCHEMA_ENV).is_some()
            && std::env::var_os(STRUCTURED_CAPTURE_ENV).is_some();
        let flavour = match (fanout, structured) {
            (true, false) => "fanout",
            (false, true) => "structured",
            (false, false) => "plain",
            (true, true) => panic!("one orchestrator environment per probe run"),
        };
        let no_extensions = probe == "no-extensions";
        let Loaded { loaded, .. } = session_through_build_factory(false, no_extensions).await;
        let llama = loaded.iter().any(|i| i == LLAMA_ID);
        let mcp = loaded.iter().any(|i| i == MCP_ID);
        assert_eq!(
            llama, !no_extensions,
            "child {flavour}, no_extensions={no_extensions}: llama.cpp is attached unless the flag \
             drops it; got {loaded:?}"
        );
        assert_eq!(
            llama, mcp,
            "child {flavour}: llama.cpp follows the MCP adapter's ambient treatment; got {loaded:?}"
        );
        // The orchestrator environment DID reach this launch, so neither extra arm is a silent
        // copy of `plain`: a fan-out child attaches the subagents extension off its authorization
        // alone, and a child carrying the structured-output pair attaches the prompt runtime —
        // neither of which a `plain` child has (`plain` loads exactly `[llama.cpp, mcp]`). A
        // misspelled or renamed variable therefore fails HERE instead of quietly re-running the
        // `plain` case three times.
        //
        // Both companions are install-gated, not ambient, so `--no-extensions` does NOT drop them:
        // that is the contrast this test is about. llama.cpp goes with the flag, these stay.
        let companion = match flavour {
            "fanout" => Some(cyrup_ext_subagents::extension::EXTENSION_ID),
            "structured" => Some(cyrup_ext_subagents::prompt_runtime::PROMPT_RUNTIME_EXTENSION_ID),
            _ => None,
        };
        if let Some(companion) = companion {
            assert!(
                loaded.iter().any(|i| i == companion),
                "child {flavour}, no_extensions={no_extensions}: `{companion}` proves the \
                 orchestrator environment reached this launch; got {loaded:?}"
            );
        } else {
            assert!(
                !loaded
                    .iter()
                    .any(|i| i == cyrup_ext_subagents::extension::EXTENSION_ID
                        || i == cyrup_ext_subagents::prompt_runtime::PROMPT_RUNTIME_EXTENSION_ID),
                "child plain: neither orchestrator extension may be present, or the two arms \
                 above prove nothing; got {loaded:?}"
            );
        }
        println!("llama-probe-ok:{flavour}:{probe}");
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

    // ============================================================================================
    // SUBA-166's blast radius — a native built-in that REFUSES to be built is quarantined, not
    // carried out of the launch path.
    //
    // `load_subagent_extension_config` refuses a `config.json` that failed validation while
    // declaring a policy key, and that refusal used to leave `attach_native_extensions` as an
    // `anyhow::Error`: no session at all, and not one of the other six built-ins attempted. pi
    // loses the one extension — the factory's throw is caught, `load.discard()`ed and recorded as
    // `Failed to load extension: <message>`
    // (`pi/packages/coding-agent/src/core/extensions/loader.ts:613-630`, `:655` @v1.0.1) —
    // and the session is built without it.
    //
    // Every test here drives the production seam, `build_factory`, and reads what the extension
    // host actually loaded plus the diagnostics the bin's checkpoint consumes.
    // ============================================================================================

    /// The subagents built-in's own id (`cyrup_ext_subagents::extension::EXTENSION_ID`) — the key
    /// the quarantine placeholder carries, and therefore the one the diagnostic names.
    const SUBAGENTS_ID: &str = cyrup_ext_subagents::extension::EXTENSION_ID;

    /// Write `<agent_dir>/subagents/config.json`. Its mere PRESENCE is the extension's install
    /// signal (`cyrup_ext_subagents::extension::is_installed`), so every case below actually
    /// reaches the subagents arm of the attach point instead of the opt-out.
    fn write_subagents_config(agent_dir: &Path, json: &str) {
        let dir = agent_dir.join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), json).unwrap();
    }

    /// A hermetic agent dir + project, prepared with `config.json`, built through `build_factory`
    /// and then through `AgentSessionRuntime::create` — the real launch sequence — returning the
    /// ids the host loaded and the diagnostics `report_runtime_diagnostics` reads in every mode.
    async fn launch_with_subagents_config(json: &str) -> (Vec<String>, Vec<RuntimeDiagnostic>) {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        write_subagents_config(&agent_dir, json);
        let (factory, target) = factory_at(&agent_dir, &cwd, false, false);
        let runtime = AgentSessionRuntime::create(factory, target)
            .await
            .expect("a refused subagents config must not abort the launch");
        let diagnostics = runtime.diagnostics().await;
        let loaded = runtime
            .session()
            .await
            .services()
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        (loaded, diagnostics)
    }

    /// A `config.json` that fails validation AND declares `authorityPolicy` — pi's #2624 scenario,
    /// the one SUBA-166 ported. The typo'd `stopRuns` action means the file cannot be honoured, and
    /// `authorityPolicy` means the all-defaults config must not stand in for it.
    const REFUSED_CONFIG: &str =
        r#"{"maxSubagentDepth": 5, "authorityPolicy": {"stopRuns": "allow"}}"#;
    /// The same file with the typo fixed: valid, policy-declaring, loads.
    const ACCEPTED_CONFIG: &str =
        r#"{"maxSubagentDepth": 5, "authorityPolicy": {"stopRun": "forbid"}}"#;

    /// THE headline. The refusal still refuses — the subagents extension is ABSENT, so the
    /// built-in defaults never stand in for the operator's `authorityPolicy` (SUBA-166 intact) —
    /// and it costs this launch that one extension and nothing else: the session is built, and the
    /// loaded set is EXACTLY the clean launch's set minus `subagents`.
    ///
    /// Asserting against the clean launch rather than a hardcoded id list is what makes "the other
    /// native extensions still attach" provable without a literal that can drift: llama.cpp, the
    /// MCP adapter and flux are all in there because the clean run loaded them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refused_subagents_config_costs_the_launch_only_that_extension() {
        let (clean, clean_diags) = launch_with_subagents_config(ACCEPTED_CONFIG).await;
        assert!(
            clean.iter().any(|i| i == SUBAGENTS_ID),
            "the control launch must actually attach subagents, or this test proves nothing; got {clean:?}"
        );
        assert!(
            clean_diags.is_empty(),
            "a valid policy-declaring config is not a diagnostic: {clean_diags:?}"
        );

        let (refused, _) = launch_with_subagents_config(REFUSED_CONFIG).await;
        let expected: Vec<String> = clean
            .iter()
            .filter(|i| i.as_str() != SUBAGENTS_ID)
            .cloned()
            .collect();
        assert_eq!(
            refused, expected,
            "a refused config must quarantine subagents and NOTHING else (clean: {clean:?})"
        );
    }

    /// The other half: the refusal is REPORTED, through the same channel every other
    /// extension-load failure uses, in pi's own message shape and marked fatal so the bin reports
    /// it and exits 1 in every mode (`main.ts:914-922`). A silent quarantine would pass the test
    /// above and be strictly worse than the launch abort it replaces.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_refusal_is_reported_as_pis_own_fatal_extension_load_failure() {
        let (_, diags) = launch_with_subagents_config(REFUSED_CONFIG).await;
        let errors: Vec<&RuntimeDiagnostic> =
            diags.iter().filter(|d| d.severity == "error").collect();
        assert_eq!(
            errors.len(),
            1,
            "expected exactly one fatal diagnostic, got {diags:?}"
        );
        let only = errors.first().expect("one error");
        assert_eq!(only.source.as_deref(), Some("extension"));
        assert!(
            only.message
                .starts_with("Failed to load extension \"subagents\": "),
            "pi's shape is `Failed to load extension \"<path>\": <err>` (main.ts:736-737); got {}",
            only.message
        );
        // The refusal's own sentence survives the frame, naming the file, the bad value and the
        // policy key — a diagnostic that only said "subagents failed" would not let an operator
        // fix the typo.
        for needle in [
            "config.json",
            "stopRuns",
            "authorityPolicy",
            "must not be silently discarded",
        ] {
            assert!(
                only.message.contains(needle),
                "the refusal's message is carried verbatim; `{needle}` is missing from {}",
                only.message
            );
        }
    }

    // ============================================================================================
    // The same quarantine at the SAME attach point, for the intercom built-in.
    //
    // Upstream's intercom factory opens with `loadConfig()` and `getAskTimeoutMs()`
    // (`pi-intercom/index.ts:647-649` @v0.16.0), both of which THROW on bad input
    // (`config.ts:98-159` and `:18`). They throw from inside the default-exported factory, which is
    // the body pi's loader wraps in `try` — so a malformed intercom config costs upstream that one
    // extension and nothing else. cyrup carried the error out of `attach_native_extensions`
    // instead, aborting the entire launch.
    // ============================================================================================

    /// The intercom built-in's own id, as the placeholder carries it.
    const INTERCOM_ID: &str = cyrup_intercom::EXTENSION_ID;

    /// Write `<agent_dir>/intercom/config.json`. As with subagents, its mere PRESENCE is the
    /// install signal (`cyrup_intercom::extension::is_installed` is
    /// `env_truthy(INSTALL_ENV_VAR) || config_path(..).exists()`), so one file both opts the
    /// extension in and supplies the bad input — the control and the refusal differ only in the
    /// file's CONTENTS, never in whether intercom was reachable at all.
    async fn launch_with_intercom_config(json: &str) -> (Vec<String>, Vec<RuntimeDiagnostic>) {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let dir = agent_dir.join("intercom");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), json).unwrap();
        let (factory, target) = factory_at(&agent_dir, &cwd, false, false);
        let runtime = AgentSessionRuntime::create(factory, target)
            .await
            .expect("a malformed intercom config must not abort the launch");
        let diagnostics = runtime.diagnostics().await;
        let loaded = runtime
            .session()
            .await
            .services()
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        (loaded, diagnostics)
    }

    /// Unparseable JSON — `load_config`'s `parse_config` arm, which carries pi's path-prefixed
    /// message (`config.ts:94-96`'s catch).
    const REFUSED_INTERCOM_CONFIG: &str = "{ this is not valid json";
    /// The same file, valid. `enabled` defaults to `true` (`config.rs:118`), so this both installs
    /// and enables intercom and the control actually attaches it.
    const ACCEPTED_INTERCOM_CONFIG: &str = r#"{"confirmSend": true}"#;

    /// The intercom headline, built the same way as the subagents one: the loaded set after the
    /// refusal is EXACTLY the control launch's set minus `cyrup-intercom`.
    ///
    /// Before the fix this test could not even reach its assertions — `AgentSessionRuntime::create`
    /// returned `Err`, because the refusal propagated out of `attach_native_extensions` and there
    /// was no session at all.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_malformed_intercom_config_costs_the_launch_only_that_extension() {
        let (clean, clean_diags) = launch_with_intercom_config(ACCEPTED_INTERCOM_CONFIG).await;
        assert!(
            clean.iter().any(|i| i == INTERCOM_ID),
            "the control launch must actually attach intercom, or this test proves nothing; got {clean:?}"
        );
        assert!(
            clean_diags.is_empty(),
            "a valid intercom config is not a diagnostic: {clean_diags:?}"
        );

        let (refused, _) = launch_with_intercom_config(REFUSED_INTERCOM_CONFIG).await;
        let expected: Vec<String> = clean
            .iter()
            .filter(|i| i.as_str() != INTERCOM_ID)
            .cloned()
            .collect();
        assert_eq!(
            refused, expected,
            "a malformed intercom config must quarantine intercom and NOTHING else (clean: {clean:?})"
        );
    }

    /// The reporting half. A silent quarantine would pass the test above while losing the operator's
    /// only clue, and would be strictly worse than the launch abort it replaces.
    ///
    /// The message must still name the config's PATH, because that is the whole point of `ICOM-044`
    /// (a malformed config errors with the path rather than failing closed silently) and the
    /// quarantine must not undo it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_intercom_refusal_is_reported_as_pis_own_fatal_extension_load_failure() {
        let (_, diags) = launch_with_intercom_config(REFUSED_INTERCOM_CONFIG).await;
        let errors: Vec<&RuntimeDiagnostic> =
            diags.iter().filter(|d| d.severity == "error").collect();
        assert_eq!(
            errors.len(),
            1,
            "expected exactly one fatal diagnostic, got {diags:?}"
        );
        let only = errors.first().expect("one error");
        assert_eq!(only.source.as_deref(), Some("extension"));
        assert!(
            only.message
                .starts_with(&format!("Failed to load extension \"{INTERCOM_ID}\": ")),
            "pi's shape is `Failed to load extension \"<id>\": <err>`; got {}",
            only.message
        );
        for needle in ["Failed to load intercom config at", "config.json"] {
            assert!(
                only.message.contains(needle),
                "ICOM-044's path-naming message is carried verbatim; `{needle}` is missing from {}",
                only.message
            );
        }
    }

    /// SUBA-166's OTHER arm, unweakened: a bad-but-present `config.json` that declares NO
    /// fail-closed key still warns and defaults (pi's `console.error` + `return {}`,
    /// `extension/config.ts:238` @v0.75.0), so the extension attaches and nothing is quarantined.
    /// This is the control that keeps "quarantine the refusal" from degrading into "quarantine
    /// every imperfect config".
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_invalid_config_declaring_no_policy_key_still_attaches_the_extension() {
        let (loaded, diags) =
            launch_with_subagents_config(r#"{"maxSubagentDepth": 5, "artifactDir": "nowhere"}"#)
                .await;
        assert!(
            loaded.iter().any(|i| i == SUBAGENTS_ID),
            "no fail-closed key is declared, so warn-and-default still applies; got {loaded:?}"
        );
        assert!(
            !diags.iter().any(|d| d.severity == "error"),
            "warn-and-default is not an extension-load failure: {diags:?}"
        );
    }

    /// The gate runs BEFORE the config is read, which is upstream's order:
    /// `registerSubagentExtension` returns on the child flag
    /// (`pi-subagents/src/extension/index.ts:315-317` @v0.75.0) and only then calls `loadConfig()`
    /// (`:325`). A process that registers nothing never opens the file, so it can never be
    /// quarantined over it — otherwise a plain subagent child would be killed by a typo in its
    /// PARENT's config, a file it was never going to read.
    ///
    /// Driven through `subagent_attachment` with the gate supplied explicitly, because the real
    /// gate reads `CYRUP_SUBAGENT_CHILD` from the process environment and `std::env::set_var` is
    /// `unsafe` (forbidden in this workspace).
    #[test]
    fn the_attach_gate_is_consulted_before_the_config_is_read() {
        use cyrup_ext_subagents::extension::RegistrationMode;

        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        write_subagents_config(&agent_dir, REFUSED_CONFIG);
        let env = cyrup_config::EnvVars {
            home: Some(agent_dir.clone()),
            ..cyrup_config::EnvVars::default()
        };
        let overrides = cyrup_config::CliConfigOverrides {
            agent_dir: Some(agent_dir.clone()),
            cwd: Some(tmp.path().join("project")),
            ..Default::default()
        };
        let dirs = cyrup_config::ConfigDirs::resolve(&overrides, &env).unwrap();

        // The gate declined (a plain subagent child): the file is not read and nothing is
        // quarantined, even though that very file WOULD be refused.
        assert!(
            matches!(
                super::subagent_attachment(&dirs, None),
                super::SubagentAttachment::Unregistered
            ),
            "a process that registers nothing must not be refused over a config it never reads"
        );
        // The same file, with the gate passing: refused.
        assert!(
            matches!(
                super::subagent_attachment(&dirs, Some(RegistrationMode::Full)),
                super::SubagentAttachment::Refused(_)
            ),
            "the same config IS refused once the gate passes, so the case above is the gate's \
             doing and not a file that loads"
        );
    }

    /// CFG-080's refusal, through the same quarantine. `decode_tool_budget_env` rejects an
    /// unauthorised `{"hard":0}` payload (`tool_budget.rs`, pi `tool-budget.ts:74-80` @v0.64.0),
    /// which used to leave `attach_native_extensions` as an `anyhow::Error` and take the launch
    /// with it. pi's `decodeToolBudgetEnv` throws out of `registerSubagentPromptRuntime`
    /// (`subagent-prompt-runtime.ts:693`) and its loader loses that ONE extension.
    ///
    /// The refusal is not weakened: the budgeted child still cannot run with its budget silently
    /// removed, because the prompt runtime is absent and the diagnostic is fatal.
    ///
    /// Env-driven, so it runs in a re-executed child — see [`run_env_probe`].
    #[test]
    fn an_undecodable_tool_budget_quarantines_only_the_prompt_runtime() {
        run_env_probe(
            "session_launch::tests::tool_budget_quarantine_probe",
            &[(
                cyrup_ext_subagents::exec::tool_budget::TOOL_BUDGET_ENV,
                r#"{"hard":0}"#,
            )],
            "tool-budget-quarantine-ok",
        );
    }

    /// The body of the probe above. `#[ignore]`d so an ordinary run cannot count a vacuous pass,
    /// and inert unless the variable it is about is actually set.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "re-executed by an_undecodable_tool_budget_quarantines_only_the_prompt_runtime"]
    async fn tool_budget_quarantine_probe() {
        let budget = cyrup_ext_subagents::exec::tool_budget::TOOL_BUDGET_ENV;
        let Ok(payload) = std::env::var(budget) else {
            return;
        };
        assert_eq!(payload, r#"{"hard":0}"#, "the probe's own payload");

        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        // No `subagents/config.json`: the subagents extension is not installed here, so the only
        // refusing built-in in this launch is the prompt runtime.
        let (factory, target) = factory_at(&agent_dir, &cwd, false, false);
        // Before the quarantine this `build_factory` returned `Err` and the line above panicked.
        let runtime = AgentSessionRuntime::create(factory, target)
            .await
            .expect("an undecodable tool budget must not abort the launch");

        let loaded: Vec<String> = runtime
            .session()
            .await
            .services()
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        let runtime_id = cyrup_ext_subagents::prompt_runtime::PROMPT_RUNTIME_EXTENSION_ID;
        assert!(
            !loaded.iter().any(|i| i == runtime_id),
            "the prompt runtime must be absent, not built with the budget dropped; got {loaded:?}"
        );
        // The other built-ins are untouched — the whole point of the quarantine.
        for id in [LLAMA_ID, MCP_ID] {
            assert!(
                loaded.iter().any(|i| i == id),
                "{id} must still attach past the quarantine; got {loaded:?}"
            );
        }

        let diags = runtime.diagnostics().await;
        let errors: Vec<&RuntimeDiagnostic> =
            diags.iter().filter(|d| d.severity == "error").collect();
        assert_eq!(errors.len(), 1, "one fatal diagnostic, got {diags:?}");
        let only = errors.first().expect("one error");
        assert_eq!(
            only.message,
            format!(
                "Failed to load extension \"{runtime_id}\": {budget}.hard must be an integer >= 1."
            ),
            "pi's frame around pi's own message"
        );
        assert_eq!(only.source.as_deref(), Some("extension"));
        println!("tool-budget-quarantine-ok");
    }

    // ============================================================================================
    // EXT-094 — the built-ins are extension resources named `builtin:<name>`: the user
    // `extensions` setting disables one by name, and `-e builtin:<name>` loads one explicitly,
    // also under `--no-extensions` (pi `core/package-manager.ts` `resolve()` /
    // `resolveExtensionSources`, `core/resource-loader.ts` `noExtensions ? cliEnabledExtensions`).
    // ============================================================================================

    /// What [`launch_with_builtins`] observed: the loaded ids, the runtime diagnostics, and every
    /// entry of the `[Extension issues]` panel (`startup_diagnostics.extensions`, as
    /// `<path>: <error>`), which also holds the non-fatal ones `diagnostics` leaves out — a `-e`
    /// path that does not exist among them.
    struct BuiltinLaunch {
        loaded: Vec<String>,
        diagnostics: Vec<RuntimeDiagnostic>,
        issues: Vec<String>,
    }

    /// A hermetic launch through `build_factory` + `AgentSessionRuntime::create`, with the global
    /// `settings.json` holding `settings` (when given), `--no-extensions` set by `no_extensions`,
    /// and `extensions` passed as `-e`.
    async fn launch_with_builtins(
        settings: Option<&str>,
        no_extensions: bool,
        extensions: &[&str],
    ) -> BuiltinLaunch {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        if let Some(json) = settings {
            std::fs::write(agent_dir.join("settings.json"), json).unwrap();
        }
        let (factory, target) = factory_with(
            Arc::new(FauxProvider::new()),
            &agent_dir,
            &cwd,
            false,
            |config| {
                config.no_extensions = no_extensions;
                config.extra_extension_paths =
                    extensions.iter().map(std::path::PathBuf::from).collect();
            },
        );
        let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
        let diagnostics = runtime.diagnostics().await;
        let session = runtime.session().await;
        let services = session.services();
        let loaded = services
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        let issues = services
            .startup_diagnostics
            .extensions
            .iter()
            .map(|d| format!("{}: {}", d.path.display(), d.error))
            .collect();
        BuiltinLaunch {
            loaded,
            diagnostics,
            issues,
        }
    }

    /// `ids` without `id`.
    fn without(ids: &[String], id: &str) -> Vec<String> {
        ids.iter().filter(|i| i.as_str() != id).cloned().collect()
    }

    /// Verify, first clause: a settings entry removes ONLY llama.cpp. The loaded set is exactly the
    /// default launch's minus `llama.cpp`, so every other built-in (mcp, tool-search, codemode's
    /// absence-or-presence included) is untouched; `-builtin:mcp` shows the name is what decides.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_settings_entry_disables_only_the_built_in_it_names() {
        let default = launch_with_builtins(None, false, &[]).await.loaded;
        assert!(
            default.iter().any(|i| i == LLAMA_ID) && default.iter().any(|i| i == MCP_ID),
            "the control launch loads both built-ins; got {default:?}"
        );
        for id in [LLAMA_ID, MCP_ID] {
            let settings = format!(r#"{{"extensions": ["-builtin:{id}"]}}"#);
            let run = launch_with_builtins(Some(&settings), false, &[]).await;
            assert_eq!(
                run.loaded,
                without(&default, id),
                "`-builtin:{id}` disables {id} and nothing else"
            );
            assert!(
                run.diagnostics.is_empty() && run.issues.is_empty(),
                "no diagnostic for a setting: {:?} {:?}",
                run.diagnostics,
                run.issues
            );
        }
    }

    /// Verify, second clause: `--no-extensions -e builtin:llama.cpp` keeps only it. The loaded set
    /// is exactly the bare `--no-extensions` launch's plus `llama.cpp`, so no other built-in comes
    /// back with it — and the `builtin:` entry never reaches the file discovery, which would report
    /// it as a missing path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_extensions_with_an_explicit_built_in_keeps_only_that_built_in() {
        let bare = launch_with_builtins(None, true, &[]).await.loaded;
        for id in BUILTIN_EXTENSIONS {
            assert!(
                !bare.iter().any(|i| i == id),
                "--no-extensions drops {id}; got {bare:?}"
            );
        }
        for id in [LLAMA_ID, cyrup_tool_search::EXTENSION_ID, MCP_ID] {
            let path = format!("builtin:{id}");
            let run = launch_with_builtins(None, true, &[&path]).await;
            let loaded = &run.loaded;
            assert!(
                loaded.iter().any(|i| i == id),
                "-ne -e {path} loads {id}; got {loaded:?}"
            );
            assert_eq!(
                without(loaded, id),
                bare,
                "-ne -e {path} adds {id} and nothing else"
            );
            assert!(
                run.diagnostics.is_empty() && run.issues.is_empty(),
                "-ne -e {path} reports nothing, and is never looked up as a file: {:?} {:?}",
                run.diagnostics,
                run.issues
            );
        }
    }

    /// `-e builtin:<name>` is in pi's `cliEnabledExtensions`, merged ahead of the settings-resolved
    /// set, so the command line loads a built-in the setting disabled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_explicit_built_in_loads_over_a_settings_entry() {
        let settings = r#"{"extensions": ["-builtin:llama.cpp"]}"#;
        let loaded = launch_with_builtins(Some(settings), false, &["builtin:llama.cpp"])
            .await
            .loaded;
        assert!(
            loaded.iter().any(|i| i == LLAMA_ID),
            "-e builtin:llama.cpp wins over -builtin:llama.cpp; got {loaded:?}"
        );
    }

    /// A name that is no built-in is pi's `Unknown built-in extension: builtin:<name>` load error,
    /// reported against that path — under `--no-extensions` too, since it is an explicit `-e` path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unknown_built_in_is_a_load_error() {
        for no_extensions in [false, true] {
            let diags = launch_with_builtins(None, no_extensions, &["builtin:nope"])
                .await
                .diagnostics;
            let errors: Vec<&RuntimeDiagnostic> =
                diags.iter().filter(|d| d.severity == "error").collect();
            assert_eq!(errors.len(), 1, "-ne={no_extensions}: {diags:?}");
            assert_eq!(
                errors.first().map(|d| d.message.as_str()),
                Some(
                    "Failed to load extension \"builtin:nope\": \
                     Unknown built-in extension: builtin:nope"
                ),
                "-ne={no_extensions}"
            );
        }
    }
}
