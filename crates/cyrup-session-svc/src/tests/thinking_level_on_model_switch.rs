//! **TUI-105-delta** — the thinking level a model switch lands on, in pi's exact precedence.
//!
//! pi `_getThinkingLevelForModelSwitch(targetModel, explicitLevel)`
//! (`packages/coding-agent/src/core/agent-session.ts:2310-2322` @v0.87.1):
//!
//! ```text
//! if (explicitLevel !== undefined) return explicitLevel;                       // :2311-2313
//! if (targetModel) { const perModel = getModelThinkingLevel(...); if (perModel !== undefined)
//!                    return perModel; }                                        // :2315-2320
//! return getDefaultThinkingLevel() ?? this.thinkingLevel ?? DEFAULT_THINKING_LEVEL;  // :2321
//! ```
//!
//! `getDefaultThinkingLevel()` is `settings.defaultThinkingLevel` and is `undefined` unless the user
//! wrote the key (`core/settings-manager.ts:799-801`), so the configured global default outranks the
//! live session level and the live session level survives a switch precisely when no default is set.
//!
//! cyrup used to skip tier 3 entirely — `apply_model_change` went straight from the per-model
//! override to the session level — and carried a `CYRUP-DELTA` saying so. That was a behavioural
//! difference, not a mechanism one: a user who had written `defaultThinkingLevel: "high"` and then
//! cycled the session to `off` kept `off` across every model switch, where pi returns to `high`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::ModelThinkingLevel;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use tempfile::TempDir;

struct Fx {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fx {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

impl Fx {
    fn write_settings(&self, json: &str) {
        std::fs::write(self.agent_dir.join("settings.json"), json).unwrap();
    }

    fn store(&self) -> Arc<dyn cyrup_config::SettingsStore> {
        Arc::new(cyrup_config::FileSettingsStore::new(
            self.agent_dir.join("settings.json"),
            self.cwd.join(".cyrup/settings.json"),
        ))
    }

    /// Two REASONING models, so a switch between them is never clamped by model support and the
    /// level the precedence chose is the level observed.
    async fn session(&self) -> crate::AgentSession {
        let mut a = FauxModelDefinition::new("faux-a");
        a.reasoning = true;
        let mut b = FauxModelDefinition::new("faux-b");
        b.reasoning = true;
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::with_config(FauxConfig {
            models: vec![a, b],
            ..FauxConfig::default()
        }));
        let mut cfg = SessionConfig::new(self.cwd.clone(), self.agent_dir.clone());
        cfg.trust_override = Some(true);
        cfg.model_pattern = Some("faux-a".to_string());
        SessionBuilder::new(provider, cfg)
            .settings_store(self.store())
            .build()
            .await
            .expect("build")
    }
}

/// **Tier 3.** With `defaultThinkingLevel` written, a model switch returns to it — it outranks the
/// live session level (`agent-session.ts:2321`, `getDefaultThinkingLevel()` BEFORE `this.thinkingLevel`).
#[tokio::test]
async fn configured_default_outranks_the_live_session_level_on_a_model_switch() {
    let fx = fixture();
    fx.write_settings("{\"defaultThinkingLevel\":\"high\"}");
    let session = fx.session().await;

    // The user cycles the live session down, then switches model for an unrelated reason.
    session
        .set_thinking_level(ModelThinkingLevel::Off)
        .await
        .unwrap();
    assert_eq!(session.thinking_level().await, ModelThinkingLevel::Off);

    session.set_model("faux-b").await.unwrap();

    assert_eq!(
        session.thinking_level().await,
        ModelThinkingLevel::High,
        "the configured `defaultThinkingLevel` is tier 3 and the session level tier 4"
    );
}

/// **Tier 4.** With NO `defaultThinkingLevel` key — the default state — the live session level
/// survives the switch (`?? this.thinkingLevel`). This is the guarantee the deleted delta claimed
/// only the reversed order could give; upstream's order gives it too, because tier 3 is absent.
#[tokio::test]
async fn unset_default_lets_the_live_session_level_survive_a_model_switch() {
    let fx = fixture();
    fx.write_settings("{}");
    let session = fx.session().await;

    session
        .set_thinking_level(ModelThinkingLevel::Low)
        .await
        .unwrap();
    session.set_model("faux-b").await.unwrap();

    assert_eq!(
        session.thinking_level().await,
        ModelThinkingLevel::Low,
        "no configured default ⇒ the in-session Shift+Tab cycle is carried across"
    );
}

/// **Tier 2 still wins.** A per-model override for the model being switched TO beats the configured
/// global default (`:2315-2320` returns before `:2321` is reached).
#[tokio::test]
async fn per_model_override_still_outranks_the_configured_default() {
    let fx = fixture();
    fx.write_settings(
        "{\"defaultThinkingLevel\":\"high\",\"modelThinkingLevels\":{\"faux/faux-b\":\"low\"}}",
    );
    let session = fx.session().await;

    session
        .set_thinking_level(ModelThinkingLevel::Off)
        .await
        .unwrap();
    session.set_model("faux-b").await.unwrap();

    assert_eq!(
        session.thinking_level().await,
        ModelThinkingLevel::Low,
        "the per-model override is tier 2"
    );
}
