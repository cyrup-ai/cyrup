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
