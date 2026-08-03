//! Network policy gate: which startup network ops are permitted (arch-07 §3.7, R-07-024…R-07-027).
//!
//! `cyrup-config` performs NO network I/O. This is the single place that decides whether the bin
//! is *allowed* to make a startup call, so DI-10 (offline-capable) is enforced in one spot.

use crate::env::{CliConfigOverrides, EnvVars};
use crate::settings::EffectiveSettings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkPolicy {
    /// `--offline` / `CYRUP_OFFLINE` → master kill switch (R-07-025/026).
    pub offline: bool,
    /// Update check, independent of telemetry (R-07-024).
    pub update_check: bool,
    /// Install/update telemetry, independent of update check (R-07-024).
    pub install_telemetry: bool,
    /// Opt-in analytics, default off (R-07-027).
    pub analytics: bool,
}

impl NetworkPolicy {
    pub fn resolve(s: &EffectiveSettings, env: &EnvVars, cli: &CliConfigOverrides) -> Self {
        let offline = cli.offline || env.offline;
        // Telemetry: env override wins over the settings value (R-07-028).
        let install_telemetry = env
            .telemetry
            .unwrap_or_else(|| s.enable_install_telemetry());
        // Update check is independent of telemetry; only the env skip toggle disables it here.
        let update_check = !env.skip_version_chk;
        let analytics = s.enable_analytics();
        Self {
            offline,
            update_check,
            install_telemetry,
            analytics,
        }
    }

    pub fn allow_update_check(&self) -> bool {
        !self.offline && self.update_check
    }

    pub fn allow_install_telemetry(&self) -> bool {
        !self.offline && self.install_telemetry
    }

    pub fn allow_analytics(&self) -> bool {
        !self.offline && self.analytics
    }

    /// Whether the runtime model-catalog refresh may reach the network (DRIFT-007).
    ///
    /// Gated on the offline kill switch ALONE, matching upstream exactly: pi's only control is
    /// `PI_OFFLINE` (`model-runtime.ts:161`, `main.ts:524,863-866`) — there is no settings key and
    /// no dedicated env var for the catalog refresh, and inventing one here would be a divergence.
    /// `--offline` / `CYRUP_OFFLINE` / `PI_OFFLINE` therefore mean NO fetch, full stop.
    ///
    /// This is only the *network* half of the gate. The persisted overlay is still loaded from disk
    /// when this returns `false` — an offline run keeps the catalogs it saw last time, and in every
    /// case the compiled-in catalogs remain the floor.
    pub fn allow_model_catalog_refresh(&self) -> bool {
        !self.offline
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn eff(json: serde_json::Value) -> EffectiveSettings {
        let obj = match json {
            serde_json::Value::Object(o) => o,
            _ => Default::default(),
        };
        EffectiveSettings::from_settings(Settings::from_map(obj))
    }

    #[test]
    fn offline_disables_everything() {
        // A-07-7
        let s = eff(serde_json::json!({ "enableInstallTelemetry": true, "enableAnalytics": true }));
        let env = EnvVars::default();
        let cli = CliConfigOverrides {
            offline: true,
            ..Default::default()
        };
        let p = NetworkPolicy::resolve(&s, &env, &cli);
        assert!(!p.allow_update_check());
        assert!(!p.allow_install_telemetry());
        assert!(!p.allow_analytics());
        assert!(!p.allow_model_catalog_refresh());
    }

    #[test]
    fn model_catalog_refresh_follows_offline_only() {
        // DRIFT-007: upstream's only control is the offline switch, so neither the telemetry toggle
        // nor the version-check skip may disable (or enable) the catalog refresh.
        let s = eff(serde_json::json!({ "enableInstallTelemetry": false }));
        let env = EnvVars {
            skip_version_chk: true,
            telemetry: Some(false),
            ..Default::default()
        };
        let p = NetworkPolicy::resolve(&s, &env, &CliConfigOverrides::default());
        assert!(p.allow_model_catalog_refresh());

        // `CYRUP_OFFLINE` / `PI_OFFLINE` (env tier) alone is enough.
        let env_offline = EnvVars {
            offline: true,
            ..Default::default()
        };
        assert!(
            !NetworkPolicy::resolve(&s, &env_offline, &CliConfigOverrides::default())
                .allow_model_catalog_refresh()
        );

        // ...and so is `--offline` (cli tier).
        let cli_offline = CliConfigOverrides {
            offline: true,
            ..Default::default()
        };
        assert!(
            !NetworkPolicy::resolve(&s, &EnvVars::default(), &cli_offline)
                .allow_model_catalog_refresh()
        );
    }

    #[test]
    fn telemetry_and_update_check_are_independent() {
        // A-07-7 / R-07-024: disabling telemetry leaves update check controllable separately.
        let s = eff(serde_json::json!({ "enableInstallTelemetry": false }));
        let env = EnvVars::default();
        let cli = CliConfigOverrides::default();
        let p = NetworkPolicy::resolve(&s, &env, &cli);
        assert!(!p.allow_install_telemetry());
        assert!(p.allow_update_check());

        // env skip-version-check disables update check but not telemetry
        let s = eff(serde_json::json!({ "enableInstallTelemetry": true }));
        let env = EnvVars {
            skip_version_chk: true,
            ..Default::default()
        };
        let p = NetworkPolicy::resolve(&s, &env, &CliConfigOverrides::default());
        assert!(!p.allow_update_check());
        assert!(p.allow_install_telemetry());
    }

    #[test]
    fn env_telemetry_overrides_settings() {
        let s = eff(serde_json::json!({ "enableInstallTelemetry": true }));
        let env = EnvVars {
            telemetry: Some(false),
            ..Default::default()
        };
        let p = NetworkPolicy::resolve(&s, &env, &CliConfigOverrides::default());
        assert!(!p.allow_install_telemetry());
    }

    #[test]
    fn analytics_default_off() {
        // R-07-027
        let s = eff(serde_json::json!({}));
        let p = NetworkPolicy::resolve(&s, &EnvVars::default(), &CliConfigOverrides::default());
        assert!(!p.allow_analytics());
    }
}
