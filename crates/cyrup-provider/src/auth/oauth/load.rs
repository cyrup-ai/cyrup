//! Per-flow OAuth module loading.
//!
//! Ports pi v0.83.0 `packages/ai/src/auth/oauth/load.ts`.
//!
//! **Mechanism divergence.** Upstream reaches each flow through a *variable* dynamic
//! `import()` (`load.ts:9-12`) so a bundler cannot follow the specifier into Node-only code
//! (`node:http` callback servers, `node:crypto` PKCE), and offers
//! `registerBundledOAuthFlowLoaders` (`load.ts:27-29`) as the escape hatch for standalone Bun
//! binaries that *are* fully bundled. Rust links statically: there is no dynamic import, and the
//! bundled-registration path is therefore the only path. The behaviour that matters — a flow is
//! resolved lazily, by id, and the caller sees a clear failure when it cannot be — is preserved:
//! [`register_bundled_oauth_flow_loaders`] fills the registry and each `load_*` returns
//! [`OAuthError::FlowUnavailable`] when its flow was never registered.
//!
//! This keeps flow modules off the resolution path's dependency graph exactly as upstream does:
//! `auth/resolve.rs` never names a flow, and a flow that no one registers costs nothing.

use super::OAuthError;
use crate::auth::OAuthAuth;
use std::sync::{Arc, RwLock};

/// Builds a flow on demand — the Rust shape of `() => OAuthAuth | Promise<OAuthAuth>`
/// (`load.ts:15-21`).
pub type OAuthFlowFactory = Arc<dyn Fn() -> Result<Arc<dyn OAuthAuth>, OAuthError> + Send + Sync>;

/// Radius is parameterised by the gateway it fronts (`load.ts:21`).
pub type RadiusFlowFactory =
    Arc<dyn Fn(RadiusOptions) -> Result<Arc<dyn OAuthAuth>, OAuthError> + Send + Sync>;

/// `{ name: string; gateway: string }` (`load.ts:21`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadiusOptions {
    pub name: String,
    pub gateway: String,
}

/// The flows pi ships (`OAuthFlowLoaders`, `load.ts:14-22`). `as_str` values are the upstream key
/// names, so an error message names the same flow pi's would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthFlowId {
    Anthropic,
    OpenAiCodex,
    GithubCopilot,
    OpenRouter,
    KimiCoding,
    Xai,
    Radius,
}

impl OAuthFlowId {
    pub fn as_str(self) -> &'static str {
        match self {
            OAuthFlowId::Anthropic => "anthropic",
            OAuthFlowId::OpenAiCodex => "openaiCodex",
            OAuthFlowId::GithubCopilot => "githubCopilot",
            OAuthFlowId::OpenRouter => "openrouter",
            OAuthFlowId::KimiCoding => "kimiCoding",
            OAuthFlowId::Xai => "xai",
            OAuthFlowId::Radius => "radius",
        }
    }
}

/// The registry filled by [`register_bundled_oauth_flow_loaders`].
///
/// Every entry is optional: partial registration is normal here, because a cyrup build may link
/// only the flows it ships. Upstream's type requires all seven because a dynamic import can
/// always fall back to the file on disk; Rust has no such fallback.
#[derive(Clone, Default)]
pub struct OAuthFlowLoaders {
    pub anthropic: Option<OAuthFlowFactory>,
    pub openai_codex: Option<OAuthFlowFactory>,
    pub github_copilot: Option<OAuthFlowFactory>,
    pub openrouter: Option<OAuthFlowFactory>,
    pub kimi_coding: Option<OAuthFlowFactory>,
    pub xai: Option<OAuthFlowFactory>,
    pub radius: Option<RadiusFlowFactory>,
}

impl OAuthFlowLoaders {
    /// Register one flow, for the common case of building the set incrementally.
    #[must_use]
    pub fn with(mut self, id: OAuthFlowId, factory: OAuthFlowFactory) -> Self {
        match id {
            OAuthFlowId::Anthropic => self.anthropic = Some(factory),
            OAuthFlowId::OpenAiCodex => self.openai_codex = Some(factory),
            OAuthFlowId::GithubCopilot => self.github_copilot = Some(factory),
            OAuthFlowId::OpenRouter => self.openrouter = Some(factory),
            OAuthFlowId::KimiCoding => self.kimi_coding = Some(factory),
            OAuthFlowId::Xai => self.xai = Some(factory),
            // Radius needs options; use `with_radius`.
            OAuthFlowId::Radius => {}
        }
        self
    }

    #[must_use]
    pub fn with_radius(mut self, factory: RadiusFlowFactory) -> Self {
        self.radius = Some(factory);
        self
    }
}

fn registry() -> &'static RwLock<OAuthFlowLoaders> {
    static REGISTRY: std::sync::OnceLock<RwLock<OAuthFlowLoaders>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(OAuthFlowLoaders::default()))
}

/// `registerBundledOAuthFlowLoaders` (`load.ts:27-29`). Replaces the registry wholesale, exactly
/// as the upstream assignment does.
pub fn register_bundled_oauth_flow_loaders(loaders: OAuthFlowLoaders) {
    if let Ok(mut slot) = registry().write() {
        *slot = loaders;
    }
}

/// The currently registered set — for status UIs that list which logins are actually available.
pub fn registered_oauth_flows() -> Vec<OAuthFlowId> {
    let Ok(loaders) = registry().read() else {
        return Vec::new();
    };
    [
        (OAuthFlowId::Anthropic, loaders.anthropic.is_some()),
        (OAuthFlowId::OpenAiCodex, loaders.openai_codex.is_some()),
        (OAuthFlowId::GithubCopilot, loaders.github_copilot.is_some()),
        (OAuthFlowId::OpenRouter, loaders.openrouter.is_some()),
        (OAuthFlowId::KimiCoding, loaders.kimi_coding.is_some()),
        (OAuthFlowId::Xai, loaders.xai.is_some()),
        (OAuthFlowId::Radius, loaders.radius.is_some()),
    ]
    .into_iter()
    .filter_map(|(id, present)| present.then_some(id))
    .collect()
}

fn load(
    id: OAuthFlowId,
    pick: fn(&OAuthFlowLoaders) -> Option<&OAuthFlowFactory>,
) -> Result<Arc<dyn OAuthAuth>, OAuthError> {
    let factory = {
        let loaders = registry()
            .read()
            .map_err(|_| OAuthError::FlowUnavailable { flow: id.as_str() })?;
        pick(&loaders).cloned()
    };
    match factory {
        Some(factory) => factory(),
        None => Err(OAuthError::FlowUnavailable { flow: id.as_str() }),
    }
}

/// `loadAnthropicOAuth` (`load.ts:31-34`).
pub fn load_anthropic_oauth() -> Result<Arc<dyn OAuthAuth>, OAuthError> {
    load(OAuthFlowId::Anthropic, |l| l.anthropic.as_ref())
}

/// `loadOpenAICodexOAuth` (`load.ts:36-39`).
pub fn load_openai_codex_oauth() -> Result<Arc<dyn OAuthAuth>, OAuthError> {
    load(OAuthFlowId::OpenAiCodex, |l| l.openai_codex.as_ref())
}

/// `loadGitHubCopilotOAuth` (`load.ts:41-44`).
pub fn load_github_copilot_oauth() -> Result<Arc<dyn OAuthAuth>, OAuthError> {
    load(OAuthFlowId::GithubCopilot, |l| l.github_copilot.as_ref())
}

/// `loadOpenRouterOAuth` (`load.ts:46-49`).
pub fn load_openrouter_oauth() -> Result<Arc<dyn OAuthAuth>, OAuthError> {
    load(OAuthFlowId::OpenRouter, |l| l.openrouter.as_ref())
}

/// `loadKimiCodingOAuth` (`load.ts:51-54`).
pub fn load_kimi_coding_oauth() -> Result<Arc<dyn OAuthAuth>, OAuthError> {
    load(OAuthFlowId::KimiCoding, |l| l.kimi_coding.as_ref())
}

/// `loadXaiOAuth` (`load.ts:56-59`).
pub fn load_xai_oauth() -> Result<Arc<dyn OAuthAuth>, OAuthError> {
    load(OAuthFlowId::Xai, |l| l.xai.as_ref())
}

/// `loadRadiusOAuth` (`load.ts:61-68`) — `createRadiusOAuth({ name, gateway })`.
pub fn load_radius_oauth(options: RadiusOptions) -> Result<Arc<dyn OAuthAuth>, OAuthError> {
    let factory = {
        let loaders = registry().read().map_err(|_| OAuthError::FlowUnavailable {
            flow: OAuthFlowId::Radius.as_str(),
        })?;
        loaders.radius.clone()
    };
    match factory {
        Some(factory) => factory(options),
        None => Err(OAuthError::FlowUnavailable {
            flow: OAuthFlowId::Radius.as_str(),
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::auth::types::{Credential, ModelAuth};
    use crate::error::AuthError;

    struct Stub(&'static str);

    /// `unwrap_err` needs `T: Debug`, and `Arc<dyn OAuthAuth>` is not one.
    fn err(result: Result<Arc<dyn OAuthAuth>, OAuthError>) -> String {
        match result {
            Ok(flow) => panic!("expected a failure, got the {} flow", flow.name()),
            Err(error) => error.to_string(),
        }
    }

    #[async_trait::async_trait]
    impl OAuthAuth for Stub {
        fn name(&self) -> &str {
            self.0
        }
        async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
            Ok(cred.clone())
        }
        async fn to_auth(&self, _cred: &Credential) -> Result<ModelAuth, AuthError> {
            Ok(ModelAuth::default())
        }
    }

    /// The registry is process-global (upstream's `bundledLoaders` is a module-level `let`), so
    /// the whole lifecycle is asserted in one test rather than racing sibling tests.
    #[test]
    fn registration_lifecycle() {
        // Nothing registered: every loader reports its flow by upstream's key name.
        register_bundled_oauth_flow_loaders(OAuthFlowLoaders::default());
        assert_eq!(registered_oauth_flows(), Vec::new());
        assert_eq!(
            err(load_anthropic_oauth()),
            "OAuth flow \"anthropic\" is not registered"
        );
        assert_eq!(
            err(load_openai_codex_oauth()),
            "OAuth flow \"openaiCodex\" is not registered"
        );
        assert_eq!(
            err(load_github_copilot_oauth()),
            "OAuth flow \"githubCopilot\" is not registered"
        );
        assert_eq!(
            err(load_openrouter_oauth()),
            "OAuth flow \"openrouter\" is not registered"
        );
        assert_eq!(
            err(load_kimi_coding_oauth()),
            "OAuth flow \"kimiCoding\" is not registered"
        );
        assert_eq!(
            err(load_xai_oauth()),
            "OAuth flow \"xai\" is not registered"
        );
        assert_eq!(
            err(load_radius_oauth(RadiusOptions {
                name: "acme".into(),
                gateway: "https://gw.example".into(),
            })),
            "OAuth flow \"radius\" is not registered"
        );

        // Register two flows plus radius.
        let seen: Arc<std::sync::Mutex<Vec<RadiusOptions>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        register_bundled_oauth_flow_loaders(
            OAuthFlowLoaders::default()
                .with(
                    OAuthFlowId::Anthropic,
                    Arc::new(|| {
                        Ok(Arc::new(Stub("Anthropic (Claude Pro/Max)")) as Arc<dyn OAuthAuth>)
                    }),
                )
                .with(
                    OAuthFlowId::Xai,
                    Arc::new(|| Ok(Arc::new(Stub("xAI OAuth")) as Arc<dyn OAuthAuth>)),
                )
                .with_radius(Arc::new(move |options: RadiusOptions| {
                    if let Ok(mut log) = recorder.lock() {
                        log.push(options.clone());
                    }
                    Ok(Arc::new(Stub("Radius")) as Arc<dyn OAuthAuth>)
                })),
        );

        assert_eq!(
            registered_oauth_flows(),
            vec![
                OAuthFlowId::Anthropic,
                OAuthFlowId::Xai,
                OAuthFlowId::Radius
            ]
        );
        assert_eq!(
            load_anthropic_oauth().unwrap().name(),
            "Anthropic (Claude Pro/Max)"
        );
        assert_eq!(load_xai_oauth().unwrap().name(), "xAI OAuth");
        // Unregistered flows still fail, so a partial build cannot silently serve the wrong flow.
        assert!(load_openrouter_oauth().is_err());

        // Radius passes its options through to the factory (`load.ts:61-68`).
        let options = RadiusOptions {
            name: "acme".into(),
            gateway: "https://gw.example".into(),
        };
        assert_eq!(load_radius_oauth(options.clone()).unwrap().name(), "Radius");
        assert_eq!(seen.lock().unwrap().as_slice(), &[options]);

        // Registration replaces wholesale, as `bundledLoaders = loaders` does.
        register_bundled_oauth_flow_loaders(OAuthFlowLoaders::default());
        assert!(load_anthropic_oauth().is_err());
    }

    /// The factory runs on each load, not once at registration — upstream's loaders are called
    /// per `loadXOAuth()`.
    #[test]
    fn factory_is_invoked_per_load() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let loaders = OAuthFlowLoaders::default().with(
            OAuthFlowId::KimiCoding,
            Arc::new(|| {
                CALLS.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::new(Stub("Kimi")) as Arc<dyn OAuthAuth>)
            }),
        );
        // Read the registry through a local load rather than the global one to avoid disturbing
        // sibling tests any more than necessary.
        let before = CALLS.load(Ordering::SeqCst);
        let factory = loaders.kimi_coding.clone().unwrap();
        let _ = factory().unwrap();
        let _ = factory().unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst) - before, 2);
    }
}
