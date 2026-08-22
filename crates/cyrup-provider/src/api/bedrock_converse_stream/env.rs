//! Environment resolution (pi `getProviderEnvValue`, `provider-env.ts:44-52`).

use crate::auth::ProviderEnv;

/// Env lookup for the resolution helpers.
///
/// `overlay` is pi's `options.env` (scoped, wins). `ambient` is the process environment; it is a
/// **test seam**: production constructs it with [`EnvSource::new`], which leaves it `None` and
/// falls through to [`std::env::var`], while the resolution tests inject a map so they never depend
/// on the ambient AWS configuration of whatever machine runs them.
#[derive(Clone, Copy, Default)]
pub(super) struct EnvSource<'a> {
    pub(super) overlay: Option<&'a ProviderEnv>,
    pub(super) ambient: Option<&'a ProviderEnv>,
}

impl<'a> EnvSource<'a> {
    pub(super) fn new(overlay: Option<&'a ProviderEnv>) -> Self {
        EnvSource {
            overlay,
            ambient: None,
        }
    }

    /// pi `getProviderEnvValue(name, env)`: the scoped overlay first, then the process env. Empty
    /// values are skipped (pi's `||` chain treats `""` as absent).
    pub(super) fn get(&self, name: &str) -> Option<String> {
        if let Some(map) = self.overlay
            && let Some(v) = map.get(name).filter(|v| !v.is_empty())
        {
            return Some(v.clone());
        }
        self.ambient(name)
    }

    /// pi `getProviderEnvValue(name)` with **no** env argument (`bedrock-converse-stream.ts:144`):
    /// the process environment only, deliberately ignoring the scoped overlay.
    pub(super) fn ambient(&self, name: &str) -> Option<String> {
        match self.ambient {
            Some(map) => map.get(name).filter(|v| !v.is_empty()).cloned(),
            None => std::env::var(name).ok().filter(|v| !v.is_empty()),
        }
    }
}
