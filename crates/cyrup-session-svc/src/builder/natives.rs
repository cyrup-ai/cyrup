//! The native built-in extension tier: which natives a session loads (SEAM-071/074), where
//! extensions are discovered from, the runtime→`ExtMode` map, and the pre-trust extension verdict
//! pass (EXT-003).

use std::path::Path;
use std::sync::Arc;

use cyrup_config::AppMode;
use cyrup_core::CancelToken;
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig, NativeExtension};

use super::SessionConfig;

/// Consult the extensions' `project_trust` verdict BEFORE the trust decision is frozen (EXT-003).
///
/// Pi does this with a deliberate throwaway load: `resource-loader.ts:378-399` calls
/// `loadProjectTrustExtensions()` (which forces `setProjectTrusted(false)` and loads only the global
/// plus CLI-configured tier), awaits `options.resolveProjectTrust({extensionsResult})`, drops that
/// set via `clearExtensionCache()`, then loads everything again against the real verdict. The
/// callback is wired at `main.ts:691-712` → `resolveProjectTrusted` (`core/project-trust.ts:46-95`),
/// which slots the extension verdict between the `--approve` override and the saved decision.
///
/// cyrup had `ExtensionHost::aggregate_project_trust`, the `project_trust` event kind, the WIT
/// `on-project-trust` export AND `cyrup_config::decide_trust_with_extension` — all of them, with
/// zero production callers, because trust was decided in builder step 1 and the `ExtensionHost` was
/// not constructed until step 4b. This is the missing call.
///
/// The pass is a THROWAWAY host: passing `project_trusted = false` is what restricts the loaded set
/// (`DiscoveredExtension::is_trusted` = `origin.is_pre_trust() || project_trusted`, loader.rs:57-60),
/// so a project-local extension cannot vote itself trusted. Natives are loaded WITHOUT the live
/// `HostServices` backend — it does not exist this early, and Pi's `projectTrustContext` likewise
/// carries only ui + cwd.
///
/// NATIVES ARE OPT-IN. Pi's module cache holds FACTORIES, not instances (`loader.ts:148,414-437`),
/// so its second pass calls the factory again against a fresh `Extension` + `ExtensionAPI`. A cyrup
/// native has no such re-instantiation: it is a process-lifetime `Arc<dyn NativeExtension>`, so
/// loading it here would call `init` TWICE ON THE SAME OBJECT. That is not theoretical —
/// `cyrup-ext-subagents`' `ChildSafe` arm spawns a detached nested-control-inbox poller from `init`,
/// and a second one would race the first over the same inbox — and the trigger is the common case
/// (any repo with a `.cyrup/` directory; a subagent child re-execs with no `--approve`). So only
/// natives that answer `NativeExtension::decides_project_trust` — whose contract is "my `init` is
/// idempotent" — take part. WASM guests always do: a guest load builds a fresh instance in a fresh
/// store, which IS Pi's fresh-per-factory-call semantics.
pub(super) async fn pre_trust_extension_verdict(
    cfg: &SessionConfig,
    cwd: &Path,
    natives: &[Arc<dyn NativeExtension>],
) -> Option<cyrup_ext::ProjectTrustDecision> {
    let (mode, has_ui) = ext_mode(cfg.app_mode);
    let host_config = HostConfig { mode, has_ui, cwd: cwd.to_path_buf() };
    #[cfg(feature = "wasm-host")]
    let host = ExtensionHost::with_wasm(host_config).ok()?;
    #[cfg(not(feature = "wasm-host"))]
    let host = ExtensionHost::new(host_config);

    // SEAM-071: a native that `--no-extensions` will not load must not vote on project trust
    // either — pi's pre-trust pass (`loadProjectTrustExtensions()`) runs over the SAME reduced set
    // its main pass does, because both read `extensionPaths` (resource-loader.ts:451-455 @v0.83.0).
    let is_subagent_child = std::env::var_os(SUBAGENT_CHILD_ENV).is_some();
    let voters = natives
        .iter()
        .filter(|e| !cfg.no_extensions || native_survives_no_extensions(e, is_subagent_child));
    for ext in voters.filter(|e| e.decides_project_trust()) {
        // A load failure in the throwaway pass must not fail the build — the real load at step 4b
        // surfaces it. Skip and keep polling the rest.
        if let Err(e) = host.load_native(ext.clone()).await {
            tracing::debug!(error = %e, "pre-trust extension load skipped");
        }
    }
    #[cfg(feature = "wasm-host")]
    {
        let roots = extension_discovery_roots(cfg);
        let deny: Arc<dyn cyrup_ext::host::HostServices> = Arc::new(cyrup_ext::DenyServices);
        // `false` = pre-trust tier only (global + CLI-configured), exactly Pi's
        // `loadProjectTrustExtensions()`.
        let _ = host.discover_and_load(&roots, false, deny).await;
    }

    let decision = host.aggregate_project_trust(&CancelToken::new()).await;
    // The host (and every instance it loaded) is dropped here — Pi's `clearExtensionCache()`.
    decision
}

/// The subagent-child marker (`cyrup_ext_subagents::spawn::nested_events::CHILD_ENV`). Read as a
/// literal rather than imported because `cyrup-session-svc` sits BELOW `cyrup-ext-subagents` in the
/// crate graph — the natives are injected into the builder from `crates/cyrup/src/main.rs`, which is
/// the only place that depends on both. See [`native_survives_no_extensions`] for why the builder
/// needs to know.
pub(super) const SUBAGENT_CHILD_ENV: &str = "CYRUP_SUBAGENT_CHILD";

/// The natives a subagent CHILD keeps across `--no-extensions`, and only a child (SEAM-071).
///
/// pi's subagent launcher passes `--no-extensions` to the child whenever the agent pins an extension
/// allowlist, and in the SAME breath re-adds three extensions as explicit `--extension <path>` args:
/// `PROMPT_RUNTIME_EXTENSION_PATH`, the fanout-child extension when the child is fanout-authorized,
/// and the resolved `@gotgenes/pi-permission-system` — `pi-subagents v0.47.1
/// src/runs/shared/pi-args.ts:413-420` (`runtimeExtensions`) emitted at `:556-560`. So in pi a child
/// under `--no-extensions` keeps exactly these three and loses everything else, pi-intercom included.
///
/// cyrup selects the same three by ENV rather than by path — `subagent_extension_for_env`,
/// `prompt_runtime_extension_for_env`, `permission_extension_for_env` in `crates/cyrup/src/main.rs`
/// — because its child-side runtime is compiled in, not a loadable file (the mechanism note at
/// `cyrup-ext-subagents/src/exec/mod.rs:1495-1499`). Env-selection IS cyrup's re-injection channel,
/// so gating these three in a child would drop what pi explicitly keeps — and for
/// `cyrup-permission-system` that is a permission gate failing OPEN, which is worse than the
/// unfiltered load SEAM-071 was filed about.
const SUBAGENT_CHILD_RUNTIME_NATIVES: [&str; 3] =
    ["cyrup-permission-system", "subagent-prompt-runtime", "subagents"];

/// Whether a native built-in survives `--no-extensions` (SEAM-071).
///
/// The discriminator is [`cyrup_ext::NativeExtension::is_ambient`], declared by each built-in on
/// itself (SEAM-074). It USED to be a hardcoded `AMBIENT_NATIVE_IDS` list here, which was unsound:
/// pi's two tiers differ by HOW an extension arrived, not by what it is called, so matching on the
/// id also caught anything that merely shared the name. That is not hypothetical — it gated
/// `build_containment_and_flag_diagnostics.rs`'s hand-injected `FailingExt { id: "subagents" }` out
/// of the load entirely, so a native init failure stopped reaching the startup panel and the exit
/// channel. An extension the embedder passed by value IS pi's inline tier by construction:
/// `loadFinalExtensionSet` calls `loadExtensionFactories` unconditionally (`resource-loader.ts:579-581`
/// @v0.83.0) over `extensionFactories = [...builtInExtensions, ...(options?.extensionFactories ?? [])]`
/// (`main.ts:523`), while only `extensionPaths` — the PATH tier — is collapsed by the flag (`:451-453`).
fn native_survives_no_extensions(ext: &Arc<dyn NativeExtension>, is_subagent_child: bool) -> bool {
    let id = ext.id();
    let id = id.as_str();
    // pi's inline-factory tier: never gated by a flag about discovery.
    if !ext.is_ambient() {
        return true;
    }
    // The ambient tier, plus pi's one carve-out: a subagent child keeps the extensions its launcher
    // re-injects by path (see [`SUBAGENT_CHILD_RUNTIME_NATIVES`]).
    is_subagent_child && SUBAGENT_CHILD_RUNTIME_NATIVES.contains(&id)
}

/// The native built-ins this session actually loads (SEAM-071). Pure, so the flag's meaning is
/// testable without standing up a session: before this existed the build loop iterated
/// `self.native_extensions` unconditionally and `--no-extensions` reached only the disk roots.
pub(super) fn natives_to_load(
    natives: Vec<Arc<dyn NativeExtension>>,
    no_extensions: bool,
    is_subagent_child: bool,
) -> Vec<Arc<dyn NativeExtension>> {
    if !no_extensions {
        return natives;
    }
    natives
        .into_iter()
        .filter(|e| {
            let keep = native_survives_no_extensions(e, is_subagent_child);
            if !keep {
                tracing::debug!(extension = %e.id(), "native built-in skipped: --no-extensions");
            }
            keep
        })
        .collect()
}

/// Build the extension discovery roots from the config (Pi `resourceLoaderOptions`
/// `additionalExtensionPaths` + `noExtensions`, main.ts:660,664). `--no-extensions`/`-ne` disables the
/// project (`<cwd>/.cyrup/extensions`) + global (`<agentDir>/extensions`) discovery roots; explicit
/// `--extension`/`-e` paths are always loaded (Pi: "explicit -e paths still work" — they are pre-trust
/// *configured* roots). Pure + side-effect-free so it is unit-testable without a wasm host.
pub(crate) fn extension_discovery_roots(cfg: &SessionConfig) -> cyrup_ext::DiscoveryRoots {
    if cfg.no_extensions {
        cyrup_ext::DiscoveryRoots {
            project_cwd: None,
            agent_dir: None,
            configured: cfg.extra_extension_paths.clone(),
        }
    } else {
        cyrup_ext::DiscoveryRoots {
            project_cwd: Some(cfg.cwd.clone()),
            agent_dir: Some(cfg.agent_dir.clone()),
            configured: cfg.extra_extension_paths.clone(),
        }
    }
}

/// Map the runtime mode to the extension `(ExtMode, has_ui)` (R-11-002).
pub(super) fn ext_mode(mode: AppMode) -> (ExtMode, bool) {
    match mode {
        AppMode::Interactive => (ExtMode::Tui, true),
        AppMode::Rpc => (ExtMode::Rpc, true),
        AppMode::Json => (ExtMode::Json, false),
        AppMode::Print => (ExtMode::Print, false),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    // ---- SEAM-071: `--no-extensions` gates the native built-ins ----------------------------
    //
    // These were RED before the fix: the build loop iterated `self.native_extensions` with no
    // reference to `cfg.no_extensions` at all (the flag reached only `extension_discovery_roots`),
    // so `cyrup --no-extensions` still loaded the permission system, subagents and intercom — and
    // an intercom load starts a detached broker, which is how the suite accumulated 13 immortal
    // broker processes per run.

    /// A minimal native whose only job is to have an id and a tier. The second field is
    /// `is_ambient`: `true` stands in for one of pi's INSTALLED packages (the PATH tier
    /// `noExtensions` collapses), `false` for its INLINE `extensionFactories` tier. Ambience is a
    /// property of the extension, never of its name — see [`super::native_survives_no_extensions`].
    struct StubNative(cyrup_core::ExtensionId, bool);

    #[async_trait::async_trait]
    impl cyrup_ext::NativeExtension for StubNative {
        fn id(&self) -> cyrup_core::ExtensionId {
            self.0.clone()
        }
        fn is_ambient(&self) -> bool {
            self.1
        }
        async fn init(
            &self,
            _api: &mut cyrup_ext::InitApi,
        ) -> Result<(), cyrup_ext::ExtError> {
            Ok(())
        }
        async fn on_event(
            &self,
            _ev: &cyrup_ext::HostEvent,
            _ctx: &cyrup_ext::HostCtx,
        ) -> cyrup_ext::HookOutcome {
            cyrup_ext::HookOutcome::Noop
        }
    }

    /// The four shipped built-ins (pi's INSTALLED-package tier) plus one embedder-supplied
    /// extension (pi's INLINE-factory tier) — the two tiers `noExtensions` treats differently.
    fn stubs() -> Vec<std::sync::Arc<dyn cyrup_ext::NativeExtension>> {
        [
            ("cyrup-permission-system", true),
            ("subagents", true),
            ("subagent-prompt-runtime", true),
            ("cyrup-intercom", true),
            ("an-embedders-own-extension", false),
        ]
        .into_iter()
        .map(|(id, ambient)| {
            std::sync::Arc::new(StubNative(cyrup_core::ExtensionId::from(id), ambient))
                as std::sync::Arc<dyn cyrup_ext::NativeExtension>
        })
        .collect()
    }

    fn ids(v: &[std::sync::Arc<dyn cyrup_ext::NativeExtension>]) -> Vec<String> {
        v.iter().map(|e| e.id().to_string()).collect()
    }

    /// Without the flag nothing changes — the whole point is that this is a FLAG, not a policy.
    #[test]
    fn every_native_loads_without_no_extensions() {
        assert_eq!(ids(&super::natives_to_load(stubs(), false, false)).len(), 5);
        assert_eq!(ids(&super::natives_to_load(stubs(), false, true)).len(), 5);
    }

    /// A ROOT session under `--no-extensions` loads none of the four shipped built-ins. pi's
    /// `noExtensions` reduces the path tier to the explicit `-e` paths (`const extensionPaths =
    /// this.noExtensions ? cliEnabledExtensions : this.mergePaths(...)`, resource-loader.ts:451-452
    /// @v0.83.0), and `@gotgenes/pi-permission-system`, pi-intercom and pi-subagents are ordinary
    /// installed packages living in exactly that tier upstream.
    #[test]
    fn no_extensions_drops_every_ambient_native_in_a_root_session() {
        let kept = ids(&super::natives_to_load(stubs(), true, false));
        assert_eq!(
            kept,
            vec!["an-embedders-own-extension".to_string()],
            "the four shipped built-ins go; the inline one stays"
        );
    }

    /// The half that is NOT gated, and the reason SEAM-071's own preferred fix would have been
    /// wrong here: pi loads its inline tier unconditionally — `loadFinalExtensionSet` calls
    /// `loadExtensionFactories(...)` with no `noExtensions` check (`resource-loader.ts:579-581`)
    /// over `[...builtInExtensions, ...(options?.extensionFactories ?? [])]` (`main.ts:523`). An
    /// extension the caller handed over by value is not something a flag about discovery is about.
    /// Ten test files in this workspace rely on exactly that combination.
    #[test]
    fn an_extension_the_embedder_passed_by_hand_survives_no_extensions() {
        for child in [false, true] {
            assert!(
                ids(&super::natives_to_load(stubs(), true, child))
                    .contains(&"an-embedders-own-extension".to_string()),
                "inline tier survives (is_subagent_child={child})"
            );
        }
    }

    /// SEAM-074, the regression that motivated it: an INLINE extension whose id happens to collide
    /// with a shipped built-in's still survives `--no-extensions`. The predicate used to match on a
    /// hardcoded id list, so a hand-injected `FailingExt { id: "subagents" }` was silently dropped
    /// from the load — which is exactly what `src/tests/build_containment_and_flag_diagnostics.rs`'s
    /// `the_failure_reaches_the_panel_and_the_exit_channel_together` does, and it went RED: the
    /// native never loaded, so its init failure reached neither the `[Extension issues]` panel nor
    /// the fatal exit channel. pi cannot have that bug — it separates the tiers by ORIGIN
    /// (`extensionPaths` is collapsed, `resource-loader.ts:451-453` @v0.83.0; `loadExtensionFactories`
    /// is not, `:579-581`), never by name.
    #[test]
    fn an_inline_extension_that_shares_a_built_ins_id_is_still_inline() {
        let inline_double = |id: &str| -> std::sync::Arc<dyn cyrup_ext::NativeExtension> {
            std::sync::Arc::new(StubNative(cyrup_core::ExtensionId::from(id), false))
        };
        for id in ["subagents", "cyrup-permission-system", "cyrup-intercom", "subagent-prompt-runtime"]
        {
            for child in [false, true] {
                assert!(
                    super::native_survives_no_extensions(&inline_double(id), child),
                    "a by-value extension named {id} is pi's inline tier and must load \
                     (is_subagent_child={child})"
                );
            }
        }
    }

    /// A subagent CHILD keeps exactly the three pi re-injects by path, and pi-intercom is not one of
    /// them: `runtimeExtensions = [PROMPT_RUNTIME_EXTENSION_PATH, fanout-child when authorized,
    /// permSystemExt]` (pi-subagents v0.47.1 `src/runs/shared/pi-args.ts:413-417`), emitted as
    /// `--extension <path>` right after the `--no-extensions` it pairs with (`:556-560`). Dropping
    /// the permission system here would be a permission gate failing OPEN.
    #[test]
    fn a_subagent_child_keeps_exactly_the_natives_pi_re_injects() {
        let kept = ids(&super::natives_to_load(stubs(), true, true));
        assert_eq!(
            kept,
            vec![
                "cyrup-permission-system".to_string(),
                "subagents".to_string(),
                "subagent-prompt-runtime".to_string(),
                "an-embedders-own-extension".to_string(),
            ],
            "load order preserved, intercom dropped"
        );
        assert!(!kept.contains(&"cyrup-intercom".to_string()), "pi re-injects no intercom");
    }

    /// The exemption is CHILD-only. A root session that happens to have the permission system
    /// installed still drops it under `--no-extensions`, exactly as pi does — pi only re-adds it
    /// from the subagent launcher, never from `main.ts`.
    #[test]
    fn the_child_exemption_does_not_leak_into_a_root_session() {
        let one = |id: &str| -> std::sync::Arc<dyn cyrup_ext::NativeExtension> {
            std::sync::Arc::new(StubNative(cyrup_core::ExtensionId::from(id), true))
        };
        for id in super::SUBAGENT_CHILD_RUNTIME_NATIVES {
            assert!(
                super::native_survives_no_extensions(&one(id), true),
                "{id} survives in a child"
            );
            assert!(
                !super::native_survives_no_extensions(&one(id), false),
                "{id} drops at the root"
            );
        }
        assert!(!super::native_survives_no_extensions(&one("cyrup-intercom"), true));
    }
}
